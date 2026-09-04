//! VOICEVOX Engine's score-query and frame-synthesis HTTP pipeline.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use auris_vocal::{SingerFrames, SingerScore};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::backend::{BackendKind, SingingBackend};
use crate::metadata::{FORMAT_VERSION, VoiceCard, VoiceInfo};
use crate::{Acceleration, SingError, validate_frames};

const NAME: &str = "VOICEVOX";

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
        let hop_length = (f64::from(config.sample_rate) / config.frame_rate).round() as u32;
        if hop_length == 0 {
            return Err(SingError::Metadata(
                "VOICEVOX frame rate is too high for the output sample rate".into(),
            ));
        }
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
            .map_err(|error| SingError::Inference(format!("VOICEVOX {endpoint}: {error}")))?;
        response
            .into_json()
            .map_err(|error| SingError::Inference(format!("VOICEVOX {endpoint}: {error}")))
    }

    fn synthesize(&self, style: u32, query: Value) -> Result<Vec<u8>, SingError> {
        let url = format!("{}/frame_synthesis?speaker={style}", self.config.url);
        let response = ureq::post(&url)
            .send_json(query)
            .map_err(|error| SingError::Inference(format!("VOICEVOX /frame_synthesis: {error}")))?;
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
        _seed: u64,
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
        if !progress(0, 2) {
            return Err(SingError::Cancelled);
        }
        let mut query = self.post_json(
            "/sing_frame_audio_query",
            style.query_style_id,
            json!({ "notes": score.notes }),
        )?;
        let f0 = query.get("f0").and_then(Value::as_array).map(Vec::len);
        let volume = query.get("volume").and_then(Value::as_array).map(Vec::len);
        if f0 != Some(frames.len()) || volume != Some(frames.len()) {
            return Err(SingError::Inference(format!(
                "VOICEVOX query returned {f0:?} pitch and {volume:?} volume frames for {} score frames",
                frames.len()
            )));
        }
        let mut pitch = frames.f0_hz.clone();
        let mut energy = frames.energy.clone();
        if let Some(first) = pitch.first_mut() {
            *first = 0.0;
        }
        if let Some(first) = energy.first_mut() {
            *first = 0.0;
        }
        query["f0"] = json!(pitch);
        query["volume"] = json!(energy);
        query["outputSamplingRate"] = json!(self.info.sample_rate);
        query["outputStereo"] = json!(false);
        if !progress(1, 2) {
            return Err(SingError::Cancelled);
        }
        let wav = self.synthesize(style.decode_style_id, query)?;
        if !progress(2, 2) {
            return Err(SingError::Cancelled);
        }
        self.decode_wav(wav)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
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

    fn wav() -> Vec<u8> {
        let samples = [0_i16, 16_384_i16];
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

    #[test]
    fn score_query_and_frame_synthesis_round_trip_through_the_engine_api() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            let (mut query, _) = listener.accept().unwrap();
            requests.push(read_request(&mut query));
            answer(
                &mut query,
                "application/json",
                br#"{"f0":[0.0,0.0],"volume":[0.0,0.0],"phonemes":[],"outputSamplingRate":24000,"outputStereo":false}"#,
            );
            let (mut synthesis, _) = listener.accept().unwrap();
            requests.push(read_request(&mut synthesis));
            answer(&mut synthesis, "audio/wav", &wav());
            requests
        });

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "auris-voicevox-{}-{unique}.voicevox.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            format!(
                r#"{{"format_version":1,"name":"Mock singer","url":"http://{address}","sample_rate":24000,"frame_rate":100.0,"styles":[{{"name":"Mock style","query_style_id":6000,"decode_style_id":3001}}]}}"#
            ),
        )
        .unwrap();
        let mut model = crate::VoiceModel::load(&path, Acceleration::Auto).unwrap();
        assert_eq!(model.backend_kind(), BackendKind::Voicevox);
        assert_eq!(model.info().speakers(), ["Mock style"]);
        let frames = SingerFrames {
            hop_seconds: 0.01,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![0, 1],
            f0_hz: vec![0.0, 440.0],
            energy: vec![0.0, 0.8],
        };
        let score = SingerScore {
            notes: vec![
                SingerNote {
                    key: None,
                    frame_length: 1,
                    lyric: String::new(),
                },
                SingerNote {
                    key: Some(69),
                    frame_length: 1,
                    lyric: "ラ".into(),
                },
            ],
        };
        let samples = model.sing_score(&frames, &score, 0, 0).unwrap();
        assert_eq!(samples, [0.0, 0.5]);
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("POST /sing_frame_audio_query?speaker=6000 "));
        assert!(requests[0].contains("\"lyric\":\"ラ\""));
        assert!(requests[1].starts_with("POST /frame_synthesis?speaker=3001 "));
        assert!(requests[1].contains("\"outputSamplingRate\":24000"));
        assert!(requests[1].contains("440.0"));
        std::fs::remove_file(path).unwrap();
    }
}
