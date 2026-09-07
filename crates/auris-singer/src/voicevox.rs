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
    normalize_long_vowels(&mut score)?;
    validate_note_boundaries(&score, padding)?;
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
            note.lyric = vowel.ok_or_else(|| SingError::Inference(format!(
                "VOICEVOX: note {} has lyric 'ー' without a preceding vowel; enter ア, イ, ウ, エ or オ",
                index + 1
            )))?.to_string();
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

/// A consonant borrows frames from the preceding event. A single frame makes the
/// Engine produce a zero-length consonant and fail with HTTP 500 (Engine 0.25.2).
fn validate_note_boundaries(score: &SingerScore, padding: u32) -> Result<(), SingError> {
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
        let Some(previous) = index.checked_sub(1).map(|index| &score.notes[index]) else {
            continue;
        };
        // The first rest is extended before transmission and already has enough room.
        if index == 1 && previous.key.is_none() && padding >= 2 {
            continue;
        }
        let starts_with_consonant = note.key.is_some()
            && auris_vocal::kana_phonemes(&note.lyric).is_some_and(|phonemes| phonemes.len() > 1);
        if previous.frame_length == 1 && starts_with_consonant {
            return Err(SingError::Inference(format!(
                "VOICEVOX: score event {} is only 1 frame before '{}'; lengthen that note or rest to at least 2 frames, or remove the short rest",
                index, note.lyric
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
        let padded = padded_score(score, padding)?;
        let query = self.post_json(
            "/sing_frame_audio_query",
            style.query_style_id,
            json!({ "notes": padded.notes }),
        )?;
        let read = |name: &str| -> Result<Vec<f64>, SingError> {
            serde_json::from_value(query[name].clone()).map_err(|_| {
                SingError::Inference(format!("VOICEVOX query has invalid {name} frames"))
            })
        };
        Ok(CurvePrediction {
            pitch_hz: Some(read("f0")?),
            energy: Some(read("volume")?),
            leading_frames: padding as usize,
            trailing_frames: padding as usize,
            context: query,
        })
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
            return Ok(Vec::new());
        }
        if !progress(0, 2) {
            return Err(SingError::Cancelled);
        }
        let decode_style = style.decode_style_id;
        let prepared = self.prepare_curves(frames, score, speaker, seed)?;
        let padding = prepared.leading_frames;
        let query_frames = prepared.pitch_hz.len();
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
        Ok(samples)
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
                "phonemes": [],
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
        let samples = model.sing_score(&frames, &score, 0, 0);
        let requests = server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        (samples, requests)
    }

    fn request_body(request: &str) -> Value {
        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
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
    fn short_internal_notes_and_rests_report_the_event_before_a_consonant() {
        for (events, index) in [
            (vec![(1, "ア"), (25, "カ")], 1),
            (vec![(25, "ア"), (1, ""), (25, "カ")], 2),
        ] {
            let error = padded_score(&test_score(&events), 94)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(&format!("event {index} is only 1 frame")),
                "{error}"
            );
            assert!(error.contains("at least 2 frames"), "{error}");
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
            padded_score(&test_score(&events), 94).unwrap();
        }
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
                    .contains("without a preceding vowel")
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
    }
}
