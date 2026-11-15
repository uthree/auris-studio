//! VOICEVOX Engine's score-query and frame-synthesis HTTP pipeline.

mod chunks;

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use auris_vocal::{SingerFrames, SingerNote, SingerScore};
use serde::Deserialize;
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use crate::backend::{BackendKind, SingingBackend};
use crate::limits::{
    MAX_COLLECTION_ITEMS, MAX_NAME_BYTES, MAX_PATH_BYTES, MAX_TEXT_BYTES, bounded_bytes,
    checked_sample_count, read_text_file, try_copy_f64, validate_audio_dimensions,
};
use crate::metadata::{VoiceCard, VoiceInfo};
use crate::{
    Acceleration, CurveGenerator, CurvePrediction, CurveSource, CurveSources, ExpressionSpan,
    SingError, validate_frames,
};

const NAME: &str = "VOICEVOX";

/// Gives consonants room before a first-beat note and the decoder context at both boundaries.
const BOUNDARY_SECONDS: f64 = 1.0;
const WAV_HEADER_ALLOWANCE: usize = 64 * 1024;

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
    for (index, pair) in score.notes.windows(2).enumerate() {
        if pair[0].key.is_some() && pair[0].frame_length == 1 && starts_with_consonant(&pair[1]) {
            return Err(SingError::InvalidLyric {
                event: origins[index],
                lyric: pair[0].lyric.clone(),
                issue: crate::LyricIssue::TooShort,
            });
        }
    }
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
        // A syllable with ん or a bare vowel tucked under it (サン, アイ) keeps most of the note
        // for its first mora, as a singer does; anything else divides evenly.
        let tucked = count == 2 && auris_vocal::tucks_under_previous(&moras[1].0);
        let lengths: Vec<u32> = if tucked {
            let first = (note.frame_length * 3)
                .div_ceil(5)
                .min(note.frame_length - 1);
            vec![first, note.frame_length - first]
        } else {
            (0..count)
                .map(|at| note.frame_length / count + u32::from(at < note.frame_length % count))
                .collect()
        };
        for ((lyric, _), frame_length) in moras.into_iter().zip(lengths) {
            origins.push(index);
            notes.push(SingerNote {
                key: note.key,
                frame_length,
                lyric,
            });
        }
    }
    score.notes = notes;
    Ok(origins)
}

/// Validate all three Engine timelines without dropping a sung phoneme.
fn validate_query_timing(query: &mut Value, expected: usize) -> Result<(), SingError> {
    let invalid = || {
        SingError::Inference(
            "VOICEVOX query has inconsistent phoneme, f0 or volume frame lengths".into(),
        )
    };
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
    for phoneme in phonemes.iter() {
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
        if length == 0 && phoneme["phoneme"] != "pau" {
            return Err(invalid());
        }
        at = end;
    }
    if at != expected {
        return Err(invalid());
    }
    phonemes.retain(|phoneme| phoneme["frame_length"].as_u64() != Some(0));
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
    bridge_rests(score, |_, length| length == 1)
}

/// Merge selected internal rests without moving the following note or the frame clock.
fn bridge_rests(
    score: &mut SingerScore,
    mut needs_room: impl FnMut(usize, u32) -> bool,
) -> Result<(), SingError> {
    let mut index = 1;
    let mut at = score
        .notes
        .first()
        .map_or(0, |note| note.frame_length as usize);
    while index + 1 < score.notes.len() {
        let note = &score.notes[index];
        let length = note.frame_length;
        if note.key.is_none()
            && note.lyric.is_empty()
            && score.notes[index - 1].key.is_some()
            && score.notes[index - 1].frame_length > 0
            && starts_with_consonant(&score.notes[index + 1])
            && needs_room(at, length)
        {
            let previous = &mut score.notes[index - 1];
            previous.frame_length = previous.frame_length.checked_add(length).ok_or_else(|| {
                SingError::Inference("VOICEVOX note before a short rest is too long".into())
            })?;
            score.notes.remove(index);
        } else {
            index += 1;
        }
        at = at
            .checked_add(length as usize)
            .ok_or_else(|| SingError::Inference("VOICEVOX score is too long".into()))?;
    }
    Ok(())
}

/// A connected query places each consonant immediately before its note's vowel onset.
fn bridge_predicted_rests(score: &mut SingerScore, query: &Value) -> Result<(), SingError> {
    let invalid = || SingError::Inference("VOICEVOX query has invalid phoneme timing".into());
    let phonemes = query["phonemes"].as_array().ok_or_else(invalid)?;
    let mut starts = BTreeMap::new();
    let mut at = 0usize;
    for phoneme in phonemes {
        let length = phoneme["frame_length"]
            .as_u64()
            .and_then(|length| usize::try_from(length).ok())
            .ok_or_else(invalid)?;
        let end = at.checked_add(length).ok_or_else(invalid)?;
        starts.insert(end, at);
        at = end;
    }
    bridge_rests(score, |start, length| {
        starts
            .get(&(start + length as usize))
            // Equality leaves no actual pause; retaining that rest only gives the
            // second prediction another opportunity to truncate the consonant.
            .is_some_and(|consonant_start| *consonant_start <= start)
    })
}

/// Split retained pauses out of the connected vowels, keeping every predicted consonant.
fn restore_predicted_rests(score: &SingerScore, query: &mut Value) -> Result<(), SingError> {
    let invalid = || SingError::Inference("VOICEVOX query cannot restore predicted rests".into());
    let mut rests = Vec::new();
    let mut at = 0usize;
    for (index, note) in score.notes.iter().enumerate() {
        let end = at
            .checked_add(note.frame_length as usize)
            .ok_or_else(invalid)?;
        if index > 0
            && note.key.is_none()
            && score.notes[index - 1].key.is_some()
            && score
                .notes
                .get(index + 1)
                .is_some_and(starts_with_consonant)
        {
            rests.push((at, end));
        }
        at = end;
    }
    let phonemes = query["phonemes"].as_array_mut().ok_or_else(invalid)?;
    let mut restored = Vec::with_capacity(phonemes.len() + rests.len());
    let mut rests = rests.into_iter().peekable();
    at = 0;
    for mut phoneme in std::mem::take(phonemes) {
        let length = phoneme["frame_length"]
            .as_u64()
            .and_then(|length| usize::try_from(length).ok())
            .ok_or_else(invalid)?;
        let end = at.checked_add(length).ok_or_else(invalid)?;
        if let Some(&(start, onset)) = rests.peek()
            && start < end
        {
            // The vowel ends where the anticipated consonant begins. Only its
            // portion inside the rest becomes pau; the consonant stays intact.
            if start < at || end >= onset {
                return Err(invalid());
            }
            if start > at {
                phoneme["frame_length"] = json!(start - at);
                restored.push(phoneme);
            }
            restored.push(json!({"phoneme": "pau", "frame_length": end - start}));
            rests.next();
        } else {
            restored.push(phoneme);
        }
        at = end;
    }
    if rests.next().is_some() {
        return Err(invalid());
    }
    *phonemes = restored;
    Ok(())
}

fn starts_with_consonant(note: &SingerNote) -> bool {
    note.key.is_some()
        && auris_vocal::kana_phonemes(&note.lyric).is_some_and(|phonemes| phonemes.len() > 1)
}

fn is_query_consonant(phoneme: &Value) -> bool {
    phoneme["phoneme"].as_str().is_some_and(|symbol| {
        !matches!(
            symbol,
            "pau" | "cl" | "N" | "a" | "e" | "i" | "o" | "u" | "A" | "E" | "I" | "O" | "U"
        )
    })
}

/// Reserve a frame for a short note's vowel when the next consonant took its whole duration.
fn preserve_syllables(query: &mut Value) -> Result<bool, SingError> {
    let invalid =
        || SingError::Inference("VOICEVOX score is too short to preserve its syllables".into());
    let phonemes = query["phonemes"].as_array_mut().ok_or_else(invalid)?;
    let mut changed = false;
    for index in 0..phonemes.len() {
        if phonemes[index]["frame_length"].as_u64() != Some(0)
            || phonemes[index]["phoneme"] == "pau"
        {
            continue;
        }
        if is_query_consonant(&phonemes[index]) {
            return Err(invalid());
        }
        let next = phonemes.get(index + 1).ok_or_else(invalid)?;
        let length = next["frame_length"]
            .as_u64()
            .filter(|length| *length > 1)
            .ok_or_else(invalid)?;
        if !is_query_consonant(next) {
            return Err(invalid());
        }
        phonemes[index]["frame_length"] = json!(1);
        phonemes[index + 1]["frame_length"] = json!(length - 1);
        changed = true;
    }
    Ok(changed)
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

pub(crate) fn loopback_url(url: &str) -> bool {
    let Some(remainder) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, suffix)) = bracketed.split_once(']') else {
            return false;
        };
        if !suffix.is_empty()
            && suffix.strip_prefix(':').is_none_or(|port| {
                port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return false;
        }
        host
    } else {
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)));
        if host.contains(':')
            || port.is_some_and(|port| {
                port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return false;
        }
        host
    };
    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

/// Keep the Engine's explanation: a bare HTTP 400 hides which lyric it refused.
fn response_error(endpoint: &str, status: u16, reader: impl Read) -> SingError {
    let mut body = String::new();
    let _ = reader.take(8192).read_to_string(&mut body);
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
    SingError::Inference(format!("VOICEVOX {endpoint}: HTTP {status}: {detail}"))
}

fn request_error(endpoint: &str, error: ureq::Error) -> SingError {
    let reason = error.to_string();
    SingError::Inference(format!(
        "VOICEVOX {endpoint}: Check that VOICEVOX Engine is running, then try again. {reason}"
    ))
}

pub(crate) struct VoicevoxBackend {
    agent: ureq::Agent,
    config: VoicevoxConfig,
    info: VoiceInfo,
    path: PathBuf,
    acceleration: Acceleration,
    automatic_access_safe: bool,
}

impl VoicevoxBackend {
    /// Observe bounded optional server metadata after synthesis; unavailable fields stay unknown.
    fn provenance_metadata(&self, endpoint: &str) -> Value {
        let metadata = (|| -> Option<Value> {
            let response = self
                .agent
                .get(format!("{}{endpoint}", self.config.url))
                .config()
                .timeout_global(Some(std::time::Duration::from_millis(500)))
                .max_redirects(0)
                .build()
                .call()
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            let bytes = bounded_bytes(
                response.into_body().into_reader(),
                1024 * 1024,
                "synthesis identity metadata",
            )
            .ok()?;
            serde_json::from_slice(&bytes).ok()
        })();
        metadata.unwrap_or(Value::Null)
    }

    pub(crate) fn load(
        path: &Path,
        acceleration: Acceleration,
        automatic: bool,
    ) -> Result<Self, SingError> {
        let raw = read_text_file(path, "VOICEVOX connection manifest")?;
        let mut config: VoicevoxConfig =
            serde_json::from_str(&raw).map_err(|error| SingError::Metadata(error.to_string()))?;
        if config.format_version != 1 {
            return Err(SingError::Metadata(format!(
                "VOICEVOX connection format {} is unsupported; this build reads 1",
                config.format_version
            )));
        }
        config.url = config.url.trim_end_matches('/').to_string();
        if config.name.trim().is_empty()
            || config.name.len() > MAX_NAME_BYTES
            || config.url.len() > MAX_PATH_BYTES
            || config.styles.is_empty()
            || config.styles.len() > MAX_COLLECTION_ITEMS
            || config
                .styles
                .iter()
                .any(|style| style.name.trim().is_empty() || style.name.len() > MAX_NAME_BYTES)
        {
            return Err(SingError::Metadata(
                "VOICEVOX names, URL, or style list exceed their practical limits".into(),
            ));
        }
        if !config.url.starts_with("http://") && !config.url.starts_with("https://") {
            return Err(SingError::Metadata(
                "VOICEVOX url must begin with http:// or https://".into(),
            ));
        }
        if !config.frame_rate.is_finite() || config.frame_rate <= 0.0 {
            return Err(SingError::Metadata(
                "VOICEVOX sample_rate and frame_rate must be positive".into(),
            ));
        }
        let hop_length = output_hop(config.sample_rate, config.frame_rate)?;
        validate_audio_dimensions(config.sample_rate, hop_length, NAME)?;
        let automatic_access_safe = loopback_url(&config.url);
        if automatic && !automatic_access_safe {
            return Err(SingError::UnsafeAutomaticAccess {
                reason: "automatic VOICEVOX synthesis requires a numeric loopback URL".into(),
            });
        }
        // A loopback manifest remains loopback only if an Engine response cannot redirect the
        // request elsewhere. Explicitly loaded remote connections retain ureq's compatibility
        // behaviour, but are marked unsafe for cache reuse by automatic work.
        let agent = if automatic_access_safe {
            ureq::Agent::config_builder()
                .max_redirects(0)
                .http_status_as_error(false)
                .build()
                .into()
        } else {
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .into()
        };
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
            sample_rate: config.sample_rate,
            hop_length,
            n_speakers: config.styles.len() as u32,
            symbols: vec!["<sil>".into(), "<unk>".into()],
            speaker_to_id,
            voice: Some(VoiceCard {
                name: config.name.clone(),
                description: "VOICEVOX Engine singing connection".into(),
                ..VoiceCard::default()
            }),
        };
        Ok(Self {
            agent,
            config,
            info,
            path: path.to_path_buf(),
            acceleration,
            automatic_access_safe,
        })
    }

    fn post_json(&self, endpoint: &str, style: u32, body: Value) -> Result<Value, SingError> {
        let url = format!("{}{endpoint}?speaker={style}", self.config.url);
        let response = self
            .agent
            .post(&url)
            .send_json(body)
            .map_err(|error| request_error(endpoint, error))?;
        if !response.status().is_success() {
            return Err(response_error(
                endpoint,
                response.status().as_u16(),
                response.into_body().into_reader(),
            ));
        }
        let bytes = bounded_bytes(
            response.into_body().into_reader(),
            MAX_TEXT_BYTES,
            "VOICEVOX JSON response",
        )?;
        serde_json::from_slice(&bytes)
            .map_err(|error| SingError::Inference(format!("VOICEVOX {endpoint}: {error}")))
    }

    fn synthesize(
        &self,
        style: u32,
        query: Value,
        expected_samples: usize,
    ) -> Result<Vec<u8>, SingError> {
        let url = format!("{}/frame_synthesis?speaker={style}", self.config.url);
        let response = self
            .agent
            .post(&url)
            .send_json(query)
            .map_err(|error| request_error("/frame_synthesis", error))?;
        if !response.status().is_success() {
            return Err(response_error(
                "/frame_synthesis",
                response.status().as_u16(),
                response.into_body().into_reader(),
            ));
        }
        let limit = expected_samples
            .checked_mul(std::mem::size_of::<f32>())
            .and_then(|bytes| bytes.checked_add(WAV_HEADER_ALLOWANCE))
            .ok_or_else(|| SingError::TooLarge {
                resource: "VOICEVOX WAV response",
                observed: None,
                limit: crate::limits::MAX_OUTPUT_SAMPLES * std::mem::size_of::<f32>()
                    + WAV_HEADER_ALLOWANCE,
            })?;
        bounded_bytes(
            response.into_body().into_reader(),
            limit,
            "VOICEVOX WAV response",
        )
    }

    fn decode_wav(&self, bytes: Vec<u8>, expected_samples: usize) -> Result<Vec<f32>, SingError> {
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
        if reader.duration() as usize != expected_samples {
            return Err(SingError::Inference(format!(
                "VOICEVOX WAV declares {} samples; expected {expected_samples}",
                reader.duration()
            )));
        }
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(expected_samples)
            .map_err(|_| SingError::Allocation {
                resource: "VOICEVOX decoded audio",
            })?;
        match spec.sample_format {
            hound::SampleFormat::Float if spec.bits_per_sample == 32 => {
                for sample in reader.samples::<f32>() {
                    let sample = sample
                        .map_err(|error| SingError::Inference(format!("VOICEVOX WAV: {error}")))?;
                    if !sample.is_finite() {
                        return Err(SingError::Inference(
                            "VOICEVOX WAV contains non-finite audio".into(),
                        ));
                    }
                    samples.push(sample);
                }
            }
            hound::SampleFormat::Int => {
                if !(1..=32).contains(&spec.bits_per_sample) {
                    return Err(SingError::Inference(format!(
                        "VOICEVOX WAV has unsupported {}-bit integer samples",
                        spec.bits_per_sample
                    )));
                }
                let scale = (1_u64 << spec.bits_per_sample.saturating_sub(1)) as f32;
                for sample in reader.samples::<i32>() {
                    let value = sample
                        .map_err(|error| SingError::Inference(format!("VOICEVOX WAV: {error}")))?;
                    samples.push(value as f32 / scale);
                }
            }
            hound::SampleFormat::Float => {
                return Err(SingError::Inference(format!(
                    "VOICEVOX WAV has unsupported {}-bit float samples",
                    spec.bits_per_sample
                )));
            }
        }
        Ok(samples)
    }
}

/// Velocity and expression scale the decoded audio, not the Engine's volume: the decoder reads
/// volume as articulation, and at half volume it sings ば as ま and ざ as だ. The gain is
/// interpolated between frame centres so a change of velocity between notes does not click.
fn apply_frame_gain(samples: &mut [f32], gain: &[f64], hop: usize) {
    let Some(last) = gain.len().checked_sub(1) else {
        return;
    };
    for (index, sample) in samples.iter_mut().enumerate() {
        let position = (index as f64 + 0.5) / hop as f64 - 0.5;
        let left = (position.max(0.0).floor() as usize).min(last);
        let right = (left + 1).min(last);
        let t = (position - left as f64).clamp(0.0, 1.0);
        *sample *= (gain[left] * (1.0 - t) + gain[right] * t) as f32;
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
        expression_spans: Vec::new(),
        context: query,
    })
}

/// Anticipated consonants carry the edits of their own note, even across another note.
fn consonant_expression_spans(
    score: &SingerScore,
    query: &Value,
    padding: usize,
) -> Result<Vec<ExpressionSpan>, SingError> {
    let invalid = || SingError::Inference("VOICEVOX query has invalid expression timing".into());
    let normalized = padded_score(score, padding as u32)?;
    let mut onsets = BTreeMap::new();
    let mut at = 0usize;
    for note in &normalized.notes {
        if starts_with_consonant(note) {
            onsets.insert(at, at.checked_sub(padding).ok_or_else(invalid)?);
        }
        at = at
            .checked_add(note.frame_length as usize)
            .ok_or_else(invalid)?;
    }
    at = 0;
    let mut spans = Vec::new();
    for phoneme in query["phonemes"].as_array().ok_or_else(invalid)? {
        let length = phoneme["frame_length"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(invalid)?;
        let end = at.checked_add(length).ok_or_else(invalid)?;
        if is_query_consonant(phoneme)
            && let Some(&control_frame) = onsets.get(&end)
        {
            spans.push(ExpressionSpan {
                frames: at..end,
                control_frame,
            });
        }
        at = end;
    }
    Ok(spans)
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
        let mut padded = padded_score(score, padding)?;
        let expected = padded
            .notes
            .iter()
            .try_fold(0usize, |sum, note| {
                sum.checked_add(note.frame_length as usize)
            })
            .ok_or_else(|| SingError::Inference("VOICEVOX score is too long".into()))?;
        checked_sample_count(
            expected,
            self.info.hop_length as usize,
            "VOICEVOX query audio",
        )?;
        // Engine 0.25.2 limits a consonant to half the immediately preceding event
        // when its predicted length exceeds that event. Even a two-frame rest can
        // therefore reduce a consonant to one frame. Predict with internal rests
        // connected first, then keep every rest that leaves the consonant enough room.
        let mut connected = padded.clone();
        bridge_rests(&mut connected, |_, _| true)?;
        let mut query = self.post_json(
            "/sing_frame_audio_query",
            style.query_style_id,
            json!({ "notes": connected.notes }),
        )?;
        let mut curves_changed = preserve_syllables(&mut query)?;
        validate_query_timing(&mut query, expected)?;
        if connected != padded {
            bridge_predicted_rests(&mut padded, &query)?;
            if padded != connected {
                restore_predicted_rests(&padded, &mut query)?;
                curves_changed = true;
            }
        }
        if curves_changed {
            // Recalculate acoustics for the corrected phoneme timeline without
            // predicting durations again and losing the correction.
            query["f0"] = self.post_json(
                "/sing_frame_f0",
                style.query_style_id,
                json!({ "score": padded, "frame_audio_query": query }),
            )?;
            validate_query_timing(&mut query, expected)?;
            query["volume"] = self.post_json(
                "/sing_frame_volume",
                style.query_style_id,
                json!({ "score": padded, "frame_audio_query": query }),
            )?;
        }
        validate_query_timing(&mut query, expected)?;
        let spans = consonant_expression_spans(score, &query, padding as usize)?;
        let mut prediction = prediction_from_query(query, padding as usize)?;
        prediction.expression_spans = spans;
        Ok(prediction)
    }
}

impl SingingBackend for VoicevoxBackend {
    fn synthesis_settings(&self, speaker: u32) -> BTreeMap<String, String> {
        let mut settings =
            BTreeMap::from([("frame_rate".into(), self.config.frame_rate.to_string())]);
        if let Some(style) = self.config.styles.get(speaker as usize) {
            settings.insert("query_style_id".into(), style.query_style_id.to_string());
            settings.insert("decode_style_id".into(), style.decode_style_id.to_string());
        }
        settings
    }
    fn synthesis_identity(&mut self, speaker: u32) -> auris_core::synthesis::SynthesisIdentity {
        use auris_core::synthesis::{
            EvidenceSource, IdentityEvidence, SynthesisComponent, SynthesisIdentity,
        };
        let declared =
            |value: Option<&str>| IdentityEvidence::new(value, EvidenceSource::ServerDeclaration);
        let version = self.provenance_metadata("/version");
        let manifest = self.provenance_metadata("/engine_manifest");
        let singers = self.provenance_metadata("/singers");
        let mut identity = SynthesisIdentity {
            backend: "voicevox".into(),
            engine: SynthesisComponent {
                id: declared(
                    manifest
                        .get("uuid")
                        .or_else(|| manifest.get("name"))
                        .and_then(Value::as_str),
                ),
                version: declared(version.as_str()),
                build: declared(manifest["build_id"].as_str()),
                ..Default::default()
            },
            ..Default::default()
        };
        if let Some(style) = self.config.styles.get(speaker as usize) {
            identity.style.id = IdentityEvidence::new(
                Some(&style.decode_style_id.to_string()),
                EvidenceSource::LocalObservation,
            );
            identity.components.insert(
                "query_style".into(),
                SynthesisComponent {
                    id: IdentityEvidence::new(
                        Some(&style.query_style_id.to_string()),
                        EvidenceSource::LocalObservation,
                    ),
                    ..Default::default()
                },
            );
            if let Some((singer, selected)) =
                singers.as_array().into_iter().flatten().find_map(|singer| {
                    singer["styles"]
                        .as_array()?
                        .iter()
                        .find(|s| s["id"].as_u64() == Some(u64::from(style.decode_style_id)))
                        .map(|s| (singer, s))
                })
            {
                identity.speaker.id = declared(singer["speaker_uuid"].as_str());
                identity.speaker.version = declared(singer["version"].as_str());
                identity.style.id = declared(Some(&style.decode_style_id.to_string()));
                identity.style.version = declared(selected["version"].as_str());
                let field = |key: &str| {
                    selected
                        .get(key)
                        .or_else(|| singer.get(key))
                        .and_then(Value::as_str)
                };
                identity.model.id = declared(field("model_id"));
                identity.model.version = declared(field("model_version"));
                identity.model.sha256 = declared(
                    field("model_sha256")
                        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())),
                );
            }
        }
        identity
    }

    fn seed_support(&self) -> auris_core::synthesis::SynthesisSeedSupport {
        auris_core::synthesis::SynthesisSeedSupport::Unsupported
    }
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

    fn automatic_access_safe(&self) -> bool {
        self.automatic_access_safe
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
        self.config
            .styles
            .get(speaker as usize)
            .ok_or(SingError::NoSuchSpeaker {
                speaker,
                count: self.info.n_speakers,
            })?;
        let score_frames: usize = score
            .notes
            .iter()
            .try_fold(0usize, |sum, note| {
                sum.checked_add(note.frame_length as usize)
            })
            .ok_or_else(|| SingError::Inference("VOICEVOX score is too long".into()))?;
        if score_frames != frames.len() {
            return Err(SingError::Inference(format!(
                "the note score covers {score_frames} frames but the curves cover {}",
                frames.len()
            )));
        }
        if frames.is_empty() {
            return Ok(crate::SingingRender::default());
        }
        let hop = self.info.hop_length as usize;
        let count = checked_sample_count(frames.len(), hop, "VOICEVOX rendered audio")?;
        let plan = chunks::ChunkPlan::new(score, frames.len(), self.config.frame_rate);
        if plan.cores.len() == 1 {
            return self.sing_chunk(frames, score, speaker, seed, progress);
        }
        let total = plan.cores.len() * 2;
        if !progress(0, total) {
            return Err(SingError::Cancelled);
        }
        // Normalize on the complete score so mora splitting, prolonged vowels and
        // lyric-error indices cannot depend on where an inference window starts.
        let normalized = padded_score(score, plan.context as u32)?;
        let mut samples = crate::limits::try_zeroed_f32(count, "VOICEVOX rendered audio")?;
        let mut pitch = Vec::new();
        pitch
            .try_reserve_exact(frames.len())
            .map_err(|_| SingError::Allocation {
                resource: "VOICEVOX displayed pitch",
            })?;
        for (index, core) in plan.cores.iter().enumerate() {
            let window = plan.window(core, frames.len());
            let local_score = chunks::slice_score(
                &normalized,
                window.start + plan.context..window.end + plan.context,
            );
            let local_frames = chunks::slice_frames(frames, window.clone());
            let render = self.sing_chunk(
                &local_frames,
                &local_score,
                speaker,
                seed,
                &mut |done, _| progress(2 * index + done, total),
            )?;
            let local_start = core.start - window.start;
            let keep_end = (core.end + plan.crossfade).min(frames.len());
            let incoming = &render.samples[local_start * hop..(keep_end - window.start) * hop];
            let overlap = if index == 0 { 0 } else { plan.crossfade * hop };
            chunks::stitch_audio(
                &mut samples[core.start * hop..keep_end * hop],
                incoming,
                overlap,
            );
            let local_pitch = render.backend_pitch.ok_or_else(|| {
                SingError::Inference("VOICEVOX chunk did not return its pitch".into())
            })?;
            pitch.extend_from_slice(&local_pitch.hz[local_start..local_start + core.len()]);
        }
        Ok(crate::SingingRender {
            provenance: None,
            samples,
            backend_pitch: Some(auris_core::SingerPitch {
                hop_seconds: frames.hop_seconds,
                hz: pitch,
            }),
        })
    }
}

impl VoicevoxBackend {
    fn sing_chunk(
        &mut self,
        frames: &SingerFrames,
        score: &SingerScore,
        speaker: u32,
        seed: u64,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<crate::SingingRender, SingError> {
        let decode_style = self
            .config
            .styles
            .get(speaker as usize)
            .ok_or(SingError::NoSuchSpeaker {
                speaker,
                count: self.info.n_speakers,
            })?
            .decode_style_id;
        let hop = self.info.hop_length as usize;
        if !progress(0, 2) {
            return Err(SingError::Cancelled);
        }
        let prepared = self.prepare_curves(frames, score, speaker, seed)?;
        let padding = prepared.leading_frames;
        let query_frames = prepared.pitch_hz.len();
        let expected = checked_sample_count(query_frames, hop, "VOICEVOX decoded audio")?;
        let pitch_end = padding
            .checked_add(frames.len())
            .filter(|end| *end <= prepared.pitch_hz.len())
            .ok_or_else(|| SingError::Inference("VOICEVOX pitch context overflow".into()))?;
        let backend_pitch = auris_core::SingerPitch {
            hop_seconds: frames.hop_seconds,
            hz: try_copy_f64(
                &prepared.pitch_hz[padding..pitch_end],
                "VOICEVOX displayed pitch",
            )?,
        };
        let mut query = prepared.context;
        query["f0"] = json!(prepared.pitch_hz);
        query["volume"] = json!(prepared.energy);
        query["outputSamplingRate"] = json!(self.info.sample_rate);
        query["outputStereo"] = json!(false);
        if !progress(1, 2) {
            return Err(SingError::Cancelled);
        }
        let wav = self.synthesize(decode_style, query, expected)?;
        if !progress(2, 2) {
            return Err(SingError::Cancelled);
        }
        let mut samples = self.decode_wav(wav, expected)?;
        if samples.len() != expected {
            return Err(SingError::Inference(format!(
                "VOICEVOX returned {} samples for {query_frames} frames; expected {expected}",
                samples.len()
            )));
        }
        apply_frame_gain(&mut samples, &prepared.gain, hop);
        // The padding belongs to this adapter, never to the track's saved score or timeline.
        let remove = checked_sample_count(padding, hop, "VOICEVOX decoder padding")?;
        let keep = checked_sample_count(frames.len(), hop, "VOICEVOX rendered audio")?;
        samples.drain(..remove);
        samples.truncate(keep);
        Ok(crate::SingingRender {
            provenance: None,
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

    #[test]
    fn a_running_engine_preserves_the_mora_inventory_across_timing_adaptation() {
        let Ok(url) = std::env::var("AURIS_VOICEVOX_TEST_URL") else {
            return;
        };
        // Japanese mora spellings accepted by Engine 0.25.2. Reference phonemes
        // come from the Engine directly, independently of Auris's kana parser.
        let moras = [
            "ァ", "ア", "ィ", "イ", "イェ", "ゥ", "ウ", "ウィ", "ウゥ", "ウェ", "ウォ", "ェ", "エ",
            "ォ", "オ", "カ", "ガ", "キ", "キィ", "キェ", "キャ", "キュ", "キョ", "ギ", "ギィ",
            "ギェ", "ギャ", "ギュ", "ギョ", "ク", "クァ", "クィ", "クゥ", "クェ", "クォ", "クヮ",
            "グ", "グァ", "グィ", "グゥ", "グェ", "グォ", "グヮ", "ケ", "ゲ", "コ", "ゴ", "サ",
            "ザ", "シ", "シェ", "シャ", "シュ", "ショ", "ジ", "ジェ", "ジャ", "ジュ", "ジョ", "ス",
            "スィ", "ズ", "ズィ", "セ", "ゼ", "ソ", "ゾ", "タ", "ダ", "チ", "チェ", "チャ", "チュ",
            "チョ", "ヂ", "ヂェ", "ヂャ", "ヂュ", "ヂョ", "ッ", "ツ", "ツァ", "ツィ", "ツェ",
            "ツォ", "ヅ", "テ", "ティ", "テェ", "テャ", "テュ", "テョ", "デ", "ディ", "デェ",
            "デャ", "デュ", "デョ", "ト", "トゥ", "ド", "ドゥ", "ナ", "ニ", "ニィ", "ニェ", "ニャ",
            "ニュ", "ニョ", "ヌ", "ネ", "ノ", "ハ", "バ", "パ", "ヒ", "ヒィ", "ヒェ", "ヒャ",
            "ヒュ", "ヒョ", "ビ", "ビィ", "ビェ", "ビャ", "ビュ", "ビョ", "ピ", "ピィ", "ピェ",
            "ピャ", "ピュ", "ピョ", "フ", "ファ", "フィ", "フェ", "フォ", "ブ", "プ", "ヘ", "ベ",
            "ペ", "ホ", "ボ", "ポ", "マ", "ミ", "ミィ", "ミェ", "ミャ", "ミュ", "ミョ", "ム", "メ",
            "モ", "ャ", "ヤ", "ュ", "ユ", "ョ", "ヨ", "ラ", "リ", "リィ", "リェ", "リャ", "リュ",
            "リョ", "ル", "レ", "ロ", "ヮ", "ワ", "ヰ", "ヱ", "ヲ", "ン", "ヴ", "ヴァ", "ヴィ",
            "ヴェ", "ヴォ", "ヴャ", "ヴュ", "ヴョ", "ヶ",
        ];
        let path = temp_connection();
        std::fs::write(
            &path,
            serde_json::to_vec(&connection(
                &url,
                24_000,
                93.75,
                json!([{"name":"Metan","query_style_id":6000,"decode_style_id":3002}]),
            ))
            .unwrap(),
        )
        .unwrap();
        let mut backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        for (length, gap) in [(24, 0), (24, 2), (24, 8), (4, 0), (2, 0)] {
            let mut events = Vec::new();
            for mora in moras {
                events.push((length, "ア"));
                if gap > 0 {
                    events.push((gap, ""));
                }
                events.extend([(24, mora), (30, "")]);
            }
            let score = test_score(&events);
            let count = score.notes.iter().map(|n| n.frame_length as usize).sum();
            let frames = SingerFrames {
                hop_seconds: backend.info.hop_seconds(),
                inventory: vec!["a".into()],
                phonemes: vec![0; count],
                f0_hz: vec![261.62555; count],
                energy: vec![0.8; count],
            };
            let mut raw = score.clone();
            let boundary = SingerNote {
                key: None,
                frame_length: 94,
                lyric: String::new(),
            };
            raw.notes.insert(0, boundary.clone());
            raw.notes.push(boundary);
            let reference = backend
                .post_json("/sing_frame_audio_query", 6000, json!(raw))
                .unwrap();
            let prediction = backend.generate_curves(&frames, &score, 0, 0).unwrap();
            let pronounced = |query: &Value| {
                query["phonemes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|p| p["phoneme"] != "pau")
                    .map(|p| p["phoneme"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                pronounced(&prediction.context),
                pronounced(&reference),
                "previous length {length}, gap {gap}"
            );
            assert!(
                prediction.context["phonemes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|p| p["frame_length"].as_u64().unwrap() > 0)
            );
            assert_eq!(prediction.pitch_hz.as_ref().unwrap().len(), count + 188);
            prediction
                .apply_expression(&frames, &score, VoicevoxBackend::SOURCES)
                .unwrap();
        }
        for mora in moras {
            let score = test_score(&[(1, "ア"), (24, mora)]);
            if starts_with_consonant(&score.notes[1]) {
                assert!(
                    matches!(
                        validate_lyrics(&score),
                        Err(SingError::InvalidLyric {
                            issue: crate::LyricIssue::TooShort,
                            ..
                        })
                    ),
                    "{mora}"
                );
            } else {
                validate_lyrics(&score).unwrap();
            }
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn anticipated_consonant_ranges_belong_to_the_incoming_notes() {
        let score = test_score(&[(4, "あ"), (3, "か")]);
        let query = json!({"phonemes":[
            {"phoneme":"pau","frame_length":1},
            {"phoneme":"a","frame_length":2},
            {"phoneme":"k","frame_length":2},
            {"phoneme":"a","frame_length":3},
            {"phoneme":"pau","frame_length":1},
        ]});
        assert_eq!(
            consonant_expression_spans(&score, &query, 1).unwrap(),
            vec![ExpressionSpan {
                frames: 3..5,
                control_frame: 4
            }]
        );
    }

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
    fn synthesis_observes_changed_server_model_with_the_same_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for revision in ["model-a", "model-b"] {
                let (mut query, _) = listener.accept().unwrap();
                let request = read_request(&mut query);
                assert!(request.starts_with("POST /sing_frame_audio_query"));
                let frames: usize = request_body(&request)["notes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n["frame_length"].as_u64().unwrap() as usize)
                    .sum();
                answer(&mut query, "application/json", json!({"f0":vec![440.0;frames],"volume":vec![1.0;frames],"phonemes":[{"phoneme":"a","frame_length":frames}],"outputSamplingRate":24000,"outputStereo":false}).to_string().as_bytes());
                let (mut synthesis, _) = listener.accept().unwrap();
                assert!(
                    read_request(&mut synthesis).starts_with("POST /frame_synthesis?speaker=3001")
                );
                answer(&mut synthesis, "audio/wav", &wav(&vec![8192; frames * 240]));
                for (endpoint, body) in [
                    ("/version", json!("engine-1")),
                    (
                        "/engine_manifest",
                        json!({"uuid":"engine-id","build_id":"cpu-fixture"}),
                    ),
                    (
                        "/singers",
                        json!([{"speaker_uuid":"speaker-id","version":"voice-1","styles":[{"id":3001,"version":"style-1","model_id":revision,"model_version":revision,"model_sha256":"b".repeat(64)}]}]),
                    ),
                ] {
                    let (mut stream, _) = listener.accept().unwrap();
                    assert!(read_request(&mut stream).starts_with(&format!("GET {endpoint} ")));
                    answer(&mut stream, "application/json", body.to_string().as_bytes());
                }
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.voicevox.json");
        std::fs::write(&path, json!({"format_version":1,"url":format!("http://{address}"),"frame_rate":100.0,"styles":[{"name":"Fixture","query_style_id":6000,"decode_style_id":3001}]}).to_string()).unwrap();
        let mut model = crate::VoiceModel::load(&path, Acceleration::Cpu).unwrap();
        let frames = SingerFrames {
            hop_seconds: 0.01,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![1, 1],
            f0_hz: vec![440.0; 2],
            energy: vec![0.8; 2],
        };
        let score = SingerScore {
            notes: vec![SingerNote {
                key: Some(69),
                frame_length: 2,
                lyric: "ア".into(),
            }],
        };
        let first = model
            .sing_render_with(&frames, &score, 0, 42, |_, _| true)
            .unwrap();
        let frozen = first.provenance.clone().unwrap();
        let second = model
            .sing_render_with(&frames, &score, 0, 43, |_, _| true)
            .unwrap()
            .provenance
            .unwrap();
        assert_eq!(frozen.identity.model.id.value.as_deref(), Some("model-a"));
        assert_eq!(second.identity.model.id.value.as_deref(), Some("model-b"));
        assert_eq!(first.provenance.as_ref(), Some(&frozen));
        assert_eq!(frozen.configuration_sha256, second.configuration_sha256);
        assert_ne!(frozen.configuration_sha256, frozen.identity.model.sha256);
        assert_eq!(
            frozen.identity.model.sha256.source,
            auris_core::synthesis::EvidenceSource::ServerDeclaration
        );
        assert_eq!(
            frozen.seed_support,
            auris_core::synthesis::SynthesisSeedSupport::Unsupported
        );
        assert_eq!(frozen.output_audio_sha256.len(), 64);
        server.join().unwrap();
    }

    #[test]
    fn long_scores_bound_every_request_and_preserve_audio_pitch_and_controls() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut lengths = Vec::new();
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                let body = request_body(&request);
                let query_request = request.starts_with("POST /sing_frame_audio_query?");
                let count = if query_request {
                    body["notes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|note| note["frame_length"].as_u64().unwrap() as usize)
                        .sum()
                } else {
                    body["f0"].as_array().unwrap().len()
                };
                // Refuse the same oversized input that would endanger a real Engine.
                if count > 1_875 {
                    write!(stream, "HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    break;
                }
                lengths.push(count);
                if query_request {
                    let query = json!({
                        "f0": vec![440.0; count],
                        "volume": vec![1.0; count],
                        "phonemes": [{"phoneme": "a", "frame_length": count}],
                    });
                    answer(
                        &mut stream,
                        "application/json",
                        &serde_json::to_vec(&query).unwrap(),
                    );
                } else {
                    assert!(request.starts_with("POST /frame_synthesis?speaker=3001"));
                    assert_eq!(body["volume"], json!(vec![1.0; count]));
                    answer(&mut stream, "audio/wav", &wav(&vec![16_384; count * 256]));
                    if body["f0"].as_array().unwrap().last().unwrap() == 880.0 {
                        break;
                    }
                }
            }
            lengths
        });
        let path = temp_connection();
        std::fs::write(
            &path,
            connection(
                &format!("http://{address}"),
                24_000,
                93.75,
                json!([{"name":"Normal", "query_style_id":6000, "decode_style_id":3001}]),
            )
            .to_string(),
        )
        .unwrap();
        let mut backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        let count = 4_000;
        let mut frames = SingerFrames {
            hop_seconds: 256.0 / 24_000.0,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![1; count],
            f0_hz: vec![440.0; count],
            energy: vec![0.5; count],
        };
        frames.f0_hz[count - 100..].fill(880.0);
        let score = SingerScore {
            notes: vec![SingerNote {
                key: Some(69),
                frame_length: count as u32,
                lyric: "ア".into(),
            }],
        };
        let original = score.clone();
        let mut progress = Vec::new();
        let rendered = backend.sing_render_with(&frames, Some(&score), 0, 0, &mut |done, total| {
            progress.push((done, total));
            true
        });
        let lengths = server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        let rendered = rendered.unwrap();
        assert!(
            lengths.len() >= 6,
            "prediction and decoding must both be split"
        );
        assert_eq!(rendered.samples, vec![0.25; count * 256]);
        assert_eq!(
            rendered.backend_pitch.unwrap().hz,
            frames
                .f0_hz
                .iter()
                .map(|hz| f64::from(*hz))
                .collect::<Vec<_>>()
        );
        assert_eq!(score, original);
        assert_eq!(progress.first().unwrap().0, 0);
        let total = progress.last().unwrap().1;
        assert_eq!(progress.last(), Some(&(total, total)));
        assert!(
            progress
                .windows(2)
                .all(|pair| pair[0].0 <= pair[1].0 && pair[0].1 == pair[1].1)
        );
    }

    fn temp_connection() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "auris-voicevox-policy-{}-{}.voicevox.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn a_long_render_can_be_cancelled_after_prediction_before_decoding() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            assert!(request.starts_with("POST /sing_frame_audio_query?"));
            let count = request_body(&request)["notes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|note| note["frame_length"].as_u64().unwrap() as usize)
                .sum::<usize>();
            let query = json!({"f0": vec![440.0; count], "volume": vec![1.0; count],
                "phonemes": [{"phoneme": "a", "frame_length": count}]});
            answer(
                &mut stream,
                "application/json",
                &serde_json::to_vec(&query).unwrap(),
            );
            listener
        });
        let path = temp_connection();
        std::fs::write(
            &path,
            connection(
                &format!("http://{address}"),
                24_000,
                93.75,
                json!([{"name":"Normal", "query_style_id":6000, "decode_style_id":3001}]),
            )
            .to_string(),
        )
        .unwrap();
        let mut backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        let frames = SingerFrames {
            hop_seconds: 256.0 / 24_000.0,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![1; 4000],
            f0_hz: vec![440.0; 4000],
            energy: vec![0.8; 4000],
        };
        let score = test_score(&[(4000, "ア")]);
        let result =
            backend.sing_render_with(&frames, Some(&score), 0, 0, &mut |done, _| done == 0);
        assert!(matches!(result, Err(SingError::Cancelled)));
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn long_scores_report_original_lyric_indices_before_contacting_the_engine() {
        let path = temp_connection();
        std::fs::write(
            &path,
            connection(
                "http://127.0.0.1:1",
                24_000,
                93.75,
                json!([{"name":"Normal", "query_style_id":6000, "decode_style_id":3001}]),
            )
            .to_string(),
        )
        .unwrap();
        let mut backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        let score = test_score(&[(2000, "コーヒー"), (2000, "unreadable")]);
        let frames = SingerFrames {
            hop_seconds: 256.0 / 24_000.0,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![1; 4000],
            f0_hz: vec![440.0; 4000],
            energy: vec![0.8; 4000],
        };
        let result = backend.sing_render_with(&frames, Some(&score), 0, 0, &mut |_, _| true);
        assert!(matches!(
            result,
            Err(SingError::InvalidLyric { event: 1, .. })
        ));
        std::fs::remove_file(path).unwrap();
    }

    fn connection(url: &str, sample_rate: u32, frame_rate: f64, styles: Value) -> Value {
        json!({
            "format_version": 1,
            "name": "Singer",
            "url": url,
            "sample_rate": sample_rate,
            "frame_rate": frame_rate,
            "styles": styles,
        })
    }

    #[test]
    fn automatic_urls_accept_only_numeric_loopback_addresses() {
        for url in [
            "http://127.0.0.1:50021",
            "https://127.255.2.3/service",
            "http://[::1]:50021",
        ] {
            assert!(crate::automatic_voicevox_url_safe(url), "{url}");
        }
        for url in [
            "http://localhost:50021",
            "http://0.0.0.0:50021",
            "http://192.168.1.2:50021",
            "http://127.0.0.1.example:50021",
            "http://127.0.0.1@example.com",
            "http://[::1].example:50021",
        ] {
            assert!(!crate::automatic_voicevox_url_safe(url), "{url}");
        }
    }

    #[test]
    fn explicit_remote_connection_is_cache_marked_and_automatic_load_refuses_it() {
        let path = temp_connection();
        let config = connection(
            "http://192.0.2.1:50021",
            24_000,
            93.75,
            json!([{"name":"Style","query_style_id":1,"decode_style_id":2}]),
        );
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let explicit = crate::VoiceModel::load(&path, Acceleration::Cpu)
            .expect("explicit connection loading performs no HTTP request");
        assert!(!explicit.automatic_access_safe());
        let error = match crate::VoiceModel::load_for_automatic_access(&path, Acceleration::Cpu) {
            Err(error) => error,
            Ok(_) => panic!("a remote Engine must be refused for automatic synthesis"),
        };
        assert!(matches!(error, SingError::UnsafeAutomaticAccess { .. }));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn voicevox_clock_and_style_boundaries_are_checked_at_load() {
        let path = temp_connection();
        let style = json!([{"name":"Style","query_style_id":1,"decode_style_id":2}]);
        for (sample_rate, frame_rate) in [(8_000, 1_000.0), (192_000, 10.0)] {
            let config = connection(
                "http://127.0.0.1:50021",
                sample_rate,
                frame_rate,
                style.clone(),
            );
            std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
            crate::VoiceModel::load(&path, Acceleration::Cpu)
                .expect("inclusive sample-rate and hop boundary");
        }
        let invalid = connection("http://127.0.0.1:50021", 192_000, 9.999, style);
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(crate::VoiceModel::load(&path, Acceleration::Cpu).is_err());

        let mut styles: Vec<_> = (0..MAX_COLLECTION_ITEMS)
            .map(
                |id| json!({"name":format!("Style {id}"),"query_style_id":id,"decode_style_id":id}),
            )
            .collect();
        let boundary = connection(
            "http://127.0.0.1:50021",
            24_000,
            93.75,
            json!(styles.clone()),
        );
        std::fs::write(&path, serde_json::to_vec(&boundary).unwrap()).unwrap();
        crate::VoiceModel::load(&path, Acceleration::Cpu).expect("inclusive style-count boundary");
        styles.push(json!({"name":"One too many","query_style_id":5000,"decode_style_id":5000}));
        let invalid = connection("http://127.0.0.1:50021", 24_000, 93.75, json!(styles));
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(crate::VoiceModel::load(&path, Acceleration::Cpu).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn wav_response_stream_is_bounded_from_the_expected_sample_count() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let path = temp_connection();
        let config = connection(
            &format!("http://{address}"),
            24_000,
            93.75,
            json!([{"name":"Style","query_style_id":1,"decode_style_id":2}]),
        );
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        let expected_samples = 1;
        let limit = expected_samples * std::mem::size_of::<f32>() + WAV_HEADER_ALLOWANCE;
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.write_all(&vec![0; limit + 1]).unwrap();
        });
        let error = backend
            .synthesize(2, json!({}), expected_samples)
            .expect_err("streamed byte after the derived limit must be rejected");
        assert!(matches!(
            error,
            SingError::TooLarge {
                resource: "VOICEVOX WAV response",
                ..
            }
        ));
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn loopback_engine_redirects_are_not_followed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let path = temp_connection();
        let config = connection(
            &format!("http://{address}"),
            24_000,
            93.75,
            json!([{"name":"Style","query_style_id":1,"decode_style_id":2}]),
        );
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/escaped\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let error = backend
            .post_json("/sing_frame_audio_query", 1, json!({}))
            .expect_err("a loopback response must not redirect even an explicitly loaded cache");
        assert!(error.to_string().contains("HTTP 302"), "{error}");
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
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
        assert_eq!(prepared.energy, [0.0, 0.04, 0.4, 0.2, 0.0, 0.0]);
        assert_eq!(prepared.gain, [0.5, 0.5, 0.5, 0.25, 0.5, 0.5]);
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
    fn predicted_consonants_bridge_insufficient_rests_but_keep_phrase_pauses() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let path = temp_connection();
        let config = connection(
            &format!("http://{address}"),
            24_000,
            93.75,
            json!([{"name":"Style","query_style_id":6000,"decode_style_id":3002}]),
        );
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        let original = test_score(&[(23, "す"), (2, ""), (25, "な"), (20, ""), (25, "か")]);
        let server = thread::spawn(move || {
            for (index, (events, phonemes)) in [
                (
                    vec![(94, ""), (25, "す"), (45, "な"), (25, "か"), (94, "")],
                    vec![
                        (82, "pau"),
                        (12, "s"),
                        (21, "u"),
                        (4, "n"),
                        (37, "a"),
                        (8, "k"),
                        (25, "a"),
                        (94, "pau"),
                    ],
                ),
                (
                    vec![
                        (94, ""),
                        (25, "す"),
                        (25, "な"),
                        (20, ""),
                        (25, "か"),
                        (94, ""),
                    ],
                    vec![
                        (82, "pau"),
                        (12, "s"),
                        (21, "u"),
                        (4, "n"),
                        (25, "a"),
                        (12, "pau"),
                        (8, "k"),
                        (25, "a"),
                        (94, "pau"),
                    ],
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                let body = request_body(&request);
                let query = json!({
                    "f0": vec![220.0; 283],
                    "volume": vec![0.5; 283],
                    "phonemes": phonemes.into_iter().map(|(length, phoneme)| json!({"phoneme":phoneme,"frame_length":length})).collect::<Vec<_>>()
                });
                let response = if index == 0 {
                    assert!(request.starts_with("POST /sing_frame_audio_query?speaker=6000"));
                    assert_eq!(body["notes"], json!(test_score(&events).notes));
                    query
                } else {
                    assert!(request.starts_with("POST /sing_frame_f0?speaker=6000"));
                    assert_eq!(body["score"]["notes"], json!(test_score(&events).notes));
                    assert_eq!(body["frame_audio_query"]["phonemes"], query["phonemes"]);
                    json!(vec![220.0; 283])
                };
                answer(
                    &mut stream,
                    "application/json",
                    &serde_json::to_vec(&response).unwrap(),
                );
            }
            let (mut stream, _) = listener.accept().unwrap();
            assert!(read_request(&mut stream).starts_with("POST /sing_frame_volume?speaker=6000"));
            answer(
                &mut stream,
                "application/json",
                &serde_json::to_vec(&vec![0.5; 283]).unwrap(),
            );
        });
        let frames = SingerFrames {
            hop_seconds: backend.info.hop_seconds(),
            inventory: vec!["a".into()],
            phonemes: vec![0; 95],
            f0_hz: vec![220.0; 95],
            energy: vec![1.0; 95],
        };
        let prediction = backend.generate_curves(&frames, &original, 0, 0).unwrap();
        server.join().unwrap();
        assert_eq!(prediction.context["phonemes"][3]["frame_length"], 4);
        assert_eq!(prediction.context["phonemes"][5]["phoneme"], "pau");
        assert_eq!(original.notes[1].frame_length, 2);
        assert_eq!(prediction.pitch_hz.unwrap().len(), 95 + 188);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn restoring_a_pause_does_not_predict_and_truncate_its_consonant_again() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let path = temp_connection();
        std::fs::write(
            &path,
            serde_json::to_vec(&connection(
                &format!("http://{address}"),
                24_000,
                93.75,
                json!([{"name":"Style","query_style_id":6000,"decode_style_id":3002}]),
            ))
            .unwrap(),
        )
        .unwrap();
        let server = thread::spawn(move || {
            // Engine 0.25.2 predicts four /t/ frames with the notes connected,
            // but clips a fresh prediction to two frames with this five-frame rest.
            let timeline = |events: &[(usize, &str)]| {
                json!({
                    "f0": vec![260.0; 243], "volume": vec![0.5; 243],
                    "phonemes": events.iter().map(|(length, phoneme)| json!({
                        "phoneme": phoneme, "frame_length": length
                    })).collect::<Vec<_>>()
                })
            };
            let (mut stream, _) = listener.accept().unwrap();
            assert!(
                read_request(&mut stream).starts_with("POST /sing_frame_audio_query?speaker=6000")
            );
            answer(
                &mut stream,
                "application/json",
                &serde_json::to_vec(&timeline(&[
                    (94, "pau"),
                    (26, "a"),
                    (4, "t"),
                    (25, "e"),
                    (94, "pau"),
                ]))
                .unwrap(),
            );
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            if request.starts_with("POST /sing_frame_audio_query") {
                answer(
                    &mut stream,
                    "application/json",
                    &serde_json::to_vec(&timeline(&[
                        (94, "pau"),
                        (25, "a"),
                        (3, "pau"),
                        (2, "t"),
                        (25, "e"),
                        (94, "pau"),
                    ]))
                    .unwrap(),
                );
                return;
            }
            assert!(request.starts_with("POST /sing_frame_f0?speaker=6000"));
            let body = request_body(&request);
            assert_eq!(
                body["score"]["notes"],
                json!(test_score(&[(94, ""), (25, "あ"), (5, ""), (25, "て"), (94, "")]).notes)
            );
            assert_eq!(
                body["frame_audio_query"]["phonemes"][2],
                json!({"phoneme":"pau","frame_length":1})
            );
            answer(
                &mut stream,
                "application/json",
                &serde_json::to_vec(&vec![261.0; 243]).unwrap(),
            );
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            assert!(request.starts_with("POST /sing_frame_volume?speaker=6000"));
            assert_eq!(
                request_body(&request)["frame_audio_query"]["f0"],
                json!(vec![261.0; 243])
            );
            answer(
                &mut stream,
                "application/json",
                &serde_json::to_vec(&vec![0.25; 243]).unwrap(),
            );
        });
        let mut backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        let score = test_score(&[(25, "あ"), (5, ""), (25, "て")]);
        let frames = SingerFrames {
            hop_seconds: backend.info.hop_seconds(),
            inventory: vec!["a".into()],
            phonemes: vec![0; 55],
            f0_hz: vec![261.62555; 55],
            energy: vec![1.0; 55],
        };
        let prediction = backend.generate_curves(&frames, &score, 0, 0).unwrap();
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        let consonant = prediction.context["phonemes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["phoneme"] == "t")
            .unwrap();
        assert_eq!(
            consonant["frame_length"], 4,
            "the pause must not consume the predicted /t/"
        );
        assert_eq!(prediction.pitch_hz.unwrap(), vec![261.0; 243]);
        assert_eq!(prediction.energy.unwrap(), vec![0.25; 243]);
    }

    #[test]
    fn restored_pauses_preserve_multiple_consonants_and_existing_vowel_gaps() {
        let score = test_score(&[
            (10, ""),
            (20, "あ"),
            (6, ""),
            (20, "て"),
            (3, ""),
            (20, "お"),
            (8, ""),
            (20, "か"),
            (10, ""),
        ]);
        let mut query = json!({"phonemes": [
            {"phoneme":"pau", "frame_length":10},
            {"phoneme":"a", "frame_length":22},
            {"phoneme":"t", "frame_length":4},
            {"phoneme":"e", "frame_length":20},
            {"phoneme":"pau", "frame_length":3},
            {"phoneme":"o", "frame_length":22},
            {"phoneme":"k", "frame_length":6},
            {"phoneme":"a", "frame_length":20},
            {"phoneme":"pau", "frame_length":10},
        ]});
        restore_predicted_rests(&score, &mut query).unwrap();
        let phonemes: Vec<_> = query["phonemes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["phoneme"].as_str().unwrap(),
                    p["frame_length"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            phonemes,
            [
                ("pau", 10),
                ("a", 20),
                ("pau", 2),
                ("t", 4),
                ("e", 20),
                ("pau", 3),
                ("o", 20),
                ("pau", 2),
                ("k", 6),
                ("a", 20),
                ("pau", 10),
            ]
        );
    }

    #[test]
    fn a_running_engine_keeps_predicted_stops_when_restoring_short_pauses() {
        let Ok(url) = std::env::var("AURIS_VOICEVOX_TEST_URL") else {
            return;
        };
        let path = temp_connection();
        std::fs::write(
            &path,
            serde_json::to_vec(&connection(
                &url,
                24_000,
                93.75,
                json!([{"name":"Metan","query_style_id":6000,"decode_style_id":3002}]),
            ))
            .unwrap(),
        )
        .unwrap();
        let mut backend = VoicevoxBackend::load(&path, Acceleration::Cpu, false).unwrap();
        for events in [
            vec![(25, "あ"), (5, ""), (25, "て")],
            vec![(24, "せ"), (2, ""), (46, "て")],
            vec![(24, "し"), (2, ""), (24, "お"), (2, ""), (24, "り")],
        ] {
            let score = test_score(&events);
            let original = score.clone();
            let count = score.notes.iter().map(|n| n.frame_length as usize).sum();
            let frames = SingerFrames {
                hop_seconds: backend.info.hop_seconds(),
                inventory: vec!["a".into()],
                phonemes: vec![0; count],
                f0_hz: vec![261.62555; count],
                energy: vec![0.8; count],
            };
            let mut connected = padded_score(&score, 94).unwrap();
            bridge_rests(&mut connected, |_, _| true).unwrap();
            let reference = backend
                .post_json(
                    "/sing_frame_audio_query",
                    6000,
                    json!({"notes": connected.notes}),
                )
                .unwrap();
            let prediction = backend.generate_curves(&frames, &score, 0, 0).unwrap();
            let stops = |query: &Value| {
                query["phonemes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|p| p["phoneme"] == "t")
                    .map(|p| p["frame_length"].as_u64().unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(stops(&prediction.context), stops(&reference), "{events:?}");
            assert!(
                !prediction.context["phonemes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["phoneme"] == "y")
            );
            assert_eq!(prediction.pitch_hz.unwrap().len(), count + 188);
            assert_eq!(score, original);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rest_bridging_uses_predicted_lengths_without_a_fixed_duration_threshold() {
        for consonant in [4, 8, 12] {
            for gap in [2, 3, 4, 7, 8, 12, 20] {
                let original = test_score(&[(25, "ア"), (gap, ""), (25, "カ")]);
                let mut outgoing = padded_score(&original, 94).unwrap();
                let query = json!({"phonemes": [
                    {"phoneme":"pau", "frame_length":94},
                    {"phoneme":"a", "frame_length":25 + gap - consonant},
                    {"phoneme":"k", "frame_length":consonant},
                    {"phoneme":"a", "frame_length":25},
                    {"phoneme":"pau", "frame_length":94}
                ]});
                bridge_predicted_rests(&mut outgoing, &query).unwrap();
                let expected = if gap <= consonant {
                    test_score(&[(94, ""), (25 + gap, "ア"), (25, "カ"), (94, "")])
                } else {
                    padded_score(&original, 94).unwrap()
                };
                assert_eq!(outgoing, expected, "gap={gap}, consonant={consonant}");
            }
        }
    }

    #[test]
    fn a_short_note_cannot_lose_its_vowel_to_the_following_consonant() {
        let mut query = json!({"phonemes":[
            {"phoneme":"pau","frame_length":94},
            {"phoneme":"a","frame_length":0},
            {"phoneme":"m","frame_length":4},
            {"phoneme":"a","frame_length":24},
            {"phoneme":"pau","frame_length":94},
        ]});
        assert!(preserve_syllables(&mut query).unwrap());
        assert_eq!(query["phonemes"][1]["frame_length"], 1);
        assert_eq!(query["phonemes"][2]["frame_length"], 3);
    }

    #[test]
    fn one_frame_before_a_consonant_reports_the_original_lyric_event() {
        let score = test_score(&[(25, "かな"), (1, "あ"), (25, "ま")]);
        let error = validate_lyrics(&score).unwrap_err();
        assert!(matches!(
            error,
            SingError::InvalidLyric {
                event: 1,
                issue: crate::LyricIssue::TooShort,
                ..
            }
        ));
    }

    #[test]
    fn multi_mora_notes_split_without_losing_lyrics_or_frames() {
        let outgoing = padded_score(&test_score(&[(25, "こーひー")]), 94).unwrap();
        assert_eq!(
            &outgoing.notes[1..5],
            test_score(&[(7, "こ"), (6, "オ"), (6, "ひ"), (6, "イ")]).notes
        );
        assert!(padded_score(&test_score(&[(1, "かな")]), 94).is_err());
    }

    #[test]
    fn a_tucked_nasal_or_vowel_leaves_most_of_the_note_to_the_syllable() {
        let outgoing = padded_score(&test_score(&[(20, "さん"), (10, "あい")]), 94).unwrap();
        assert_eq!(
            &outgoing.notes[1..5],
            test_score(&[(12, "さ"), (8, "ん"), (6, "あ"), (4, "い")]).notes
        );
    }

    #[test]
    fn malformed_engine_phoneme_lengths_are_rejected_before_synthesis() {
        for length in [json!(-1), json!(2), json!(1.5), json!(null)] {
            let mut query = json!({"f0": [100.0], "volume": [1.0], "phonemes": [{"phoneme": "a", "frame_length": length}]});
            assert!(validate_query_timing(&mut query, 1).is_err());
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
            let score = auris_vocal::render_score(
                &track,
                &TempoMap::constant(bpm),
                &auris_core::Tuning::standard(),
            );
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
    fn short_internal_notes_are_checked_without_deleting_query_frames() {
        for events in [
            vec![(1, "ア"), (25, "カ")],
            vec![(25, "ア"), (1, "イ"), (25, "カ")],
        ] {
            assert!(padded_score(&test_score(&events), 94).is_err());
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
            assert!(padded_score(&test_score(&events), 94).is_ok());
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
        let error =
            response_error("/sing_frame_audio_query", 400, std::io::Cursor::new(body)).to_string();
        assert!(error.contains("HTTP 400"));
        assert!(error.contains("lyricが不正です: ー"));
    }

    #[test]
    fn score_query_and_frame_synthesis_preserve_timeline_after_boundary_padding() {
        for (frame_rate, padding, hop) in [(93.75, 94, 256), (100.0, 100, 240)] {
            let (samples, requests) = mock_round_trip(frame_rate, padding, true, false);
            let samples = samples.unwrap();
            assert_eq!(samples.len(), 2 * hop);
            // The note's 0.8 velocity reaches back over the opening rest, scaling the audio.
            let close = |sample: &f32, expected: f32| (sample - expected).abs() < 1e-6;
            assert!(samples[..hop].iter().all(|sample| close(sample, 0.2)));
            assert!(samples[hop..].iter().all(|sample| close(sample, 0.4)));
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
            energy[padding + 1] = 1.0;
            assert_eq!(query["f0"], json!(pitch));
            assert_eq!(query["volume"], json!(energy));
        }
    }

    #[test]
    fn a_score_without_boundary_rests_keeps_its_first_pitch_and_energy() {
        let (samples, requests) = mock_round_trip(93.75, 94, false, false);
        let samples = samples.unwrap();
        assert_eq!(samples.len(), 2 * 256);
        // Velocities 0.4 then 0.8 scale the decoded 0.25 and 0.5, meeting in a ramp.
        assert!((samples[0] - 0.1).abs() < 1e-6);
        assert!((samples[511] - 0.4).abs() < 1e-6);
        let score = request_body(&requests[0]);
        assert_eq!(
            score["notes"][0],
            json!({"key": null, "frame_length": 94, "lyric": ""})
        );
        assert_eq!(score["notes"][1]["lyric"], "ア");
        let query = request_body(&requests[1]);
        assert_eq!(query["f0"][94], json!(220.0_f32));
        assert_eq!(
            query["volume"][94],
            json!(1.0),
            "the Engine's volume is articulation, not a fader"
        );
    }

    #[test]
    fn frame_gain_ramps_between_frame_centres_instead_of_stepping() {
        let mut samples = vec![1.0_f32; 8];
        apply_frame_gain(&mut samples, &[0.0, 1.0], 4);
        assert_eq!(
            samples,
            [0.0, 0.0, 0.125, 0.375, 0.625, 0.875, 1.0, 1.0],
            "constant at each frame's outer half, linear between the centres"
        );
        let mut samples = vec![0.5_f32; 6];
        apply_frame_gain(&mut samples, &[0.5; 3], 2);
        assert_eq!(samples, [0.25; 6]);
        apply_frame_gain(&mut samples, &[], 2);
        assert_eq!(samples, [0.25; 6]);
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
            vec![(25, "カ"), (1, ""), (25, "キ")],
            vec![(23, "す"), (2, ""), (25, "な")],
            vec![(23, "な"), (2, ""), (25, "つ")],
            vec![(25, "こーひー")],
            vec![(25, "シァ"), (25, "フゥ"), (25, "じぃ")],
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
