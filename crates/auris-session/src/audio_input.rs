//! Explicit lossless audio input agreements for audio a model receives: attachments and auditions.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

/// An explicitly agreed WAV input contract. Mismatches fail instead of converting audio.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioInputContract {
    /// Accepted sample rates in Hz.
    pub sample_rates: Vec<u32>,
    /// Channel order: either ["mono"] or ["left", "right"].
    pub channel_order: Vec<String>,
    /// Accepted lossless WAV representations: pcm16, pcm24, float32.
    pub sample_formats: Vec<String>,
    /// Maximum continuous duration per input, 1..300 seconds.
    pub max_seconds: u32,
    /// Maximum container bytes per input.
    pub max_bytes: u64,
    /// Maximum combined container bytes per request, at most 512 MiB.
    pub max_total_bytes: u64,
    /// Number of complete waveforms accepted in one context, 1..2.
    pub max_inputs: u8,
    /// True only when both inputs are actually compared within one model context.
    pub simultaneous_comparison: bool,
}

impl AudioInputContract {
    /// Read an explicit JSON agreement from AURIS_AUDIO_INPUT_CONTRACT; absent retains legacy behavior.
    pub fn from_env() -> Result<Option<Self>, String> {
        match std::env::var("AURIS_AUDIO_INPUT_CONTRACT") {
            Ok(value) => {
                let contract: Self = serde_json::from_str(&value)
                    .map_err(|e| format!("Invalid AURIS_AUDIO_INPUT_CONTRACT: {e}"))?;
                contract.validate()?;
                Ok(Some(contract))
            }
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => Err("AURIS_AUDIO_INPUT_CONTRACT must be valid UTF-8 JSON".into()),
        }
    }

    /// Validate bounded capabilities before opening any audio.
    pub fn validate(&self) -> Result<(), String> {
        if self.sample_rates.is_empty()
            || self.sample_rates.len() > 16
            || self
                .sample_rates
                .iter()
                .any(|r| !(8000..=192000).contains(r))
            || !(self.channel_order == ["mono"] || self.channel_order == ["left", "right"])
            || self.sample_formats.is_empty()
            || self.sample_formats.len() > 3
            || self
                .sample_formats
                .iter()
                .any(|s| !matches!(s.as_str(), "pcm16" | "pcm24" | "float32"))
            || !(1..=300).contains(&self.max_seconds)
            || self.max_bytes == 0
            || self.max_bytes > self.max_total_bytes
            || self.max_total_bytes > 512 * 1024 * 1024
            || !(1..=2).contains(&self.max_inputs)
            || (self.simultaneous_comparison && self.max_inputs != 2)
        {
            return Err("Invalid or excessive audio input capabilities".into());
        }
        Ok(())
    }

    /// Check exact original WAV bytes and finite decoded samples without modifying the signal.
    /// Returns transfer metadata; bytes are sent unchanged, including over-range finite floats.
    pub fn inspect(&self, path: &Path, bytes: u64) -> Result<serde_json::Value, String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        self.inspect_reader(std::io::BufReader::new(file), bytes)
    }

    /// [`Self::inspect`] for WAV bytes already in memory, such as a rendered audition.
    pub fn inspect_bytes(&self, wav: &[u8]) -> Result<serde_json::Value, String> {
        let mut report = self.inspect_reader(std::io::Cursor::new(wav), wav.len() as u64)?;
        report["sha256"] = hex::encode(Sha256::digest(wav)).into();
        Ok(report)
    }

    fn inspect_reader(
        &self,
        reader: impl std::io::Read,
        bytes: u64,
    ) -> Result<serde_json::Value, String> {
        self.validate()?;
        if bytes > self.max_bytes {
            return Err(
                "Audio exceeds the agreed per-input byte limit; no conversion was performed".into(),
            );
        }
        let mut wav = hound::WavReader::new(reader)
            .map_err(|e| format!("Agreed audio input requires a lossless WAV: {e}"))?;
        let spec = wav.spec();
        let format = match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Float, 32) => "float32",
            (hound::SampleFormat::Int, 24) => "pcm24",
            (hound::SampleFormat::Int, 16) => "pcm16",
            _ => return Err("Unsupported agreed WAV sample representation".into()),
        };
        let frames = u64::from(wav.duration());
        if !wav.len().is_multiple_of(u32::from(spec.channels)) {
            return Err("WAV has an incomplete channel frame".into());
        }
        if !self.sample_rates.contains(&spec.sample_rate)
            || spec.channels as usize != self.channel_order.len()
            || !self.sample_formats.iter().any(|s| s == format)
            || frames == 0
            || frames > u64::from(spec.sample_rate) * u64::from(self.max_seconds)
        {
            return Err("Audio rate, channels, sample format or duration differs from the agreed contract; no resampling, downmixing or truncation was performed".into());
        }
        if spec.sample_format == hound::SampleFormat::Float {
            for sample in wav.samples::<f32>() {
                if !sample.map_err(|e| e.to_string())?.is_finite() {
                    return Err("Audio contains non-finite samples".into());
                }
            }
        } else {
            for sample in wav.samples::<i32>() {
                sample.map_err(|e| e.to_string())?;
            }
        }
        Ok(
            json!({"sample_rate":spec.sample_rate,"channel_order":self.channel_order,"sample_format":format,
            "frames":frames,"seconds":frames as f64/f64::from(spec.sample_rate),"bytes":bytes,
            "gain_db":0.0,"resampled":false,"channel_conversion":false,"truncated":false}),
        )
    }

    /// Validate and read a fixed regular file within the negotiated byte budget.
    pub fn read(
        &self,
        path: &Path,
        remaining: u64,
    ) -> Result<(Vec<u8>, serde_json::Value), String> {
        use std::io::Read;
        self.validate()?;
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() > remaining.min(self.max_bytes) {
            return Err("Audio exceeds the agreed request byte limit".into());
        }
        // Validate the same immutable bytes that reach the model, closing file replacement races.
        let mut bytes = Vec::new();
        file.take(remaining.min(self.max_bytes) + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > remaining.min(self.max_bytes) {
            return Err("Audio grew beyond the agreed byte limit".into());
        }
        let mut snapshot = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        use std::io::Write;
        snapshot.write_all(&bytes).map_err(|e| e.to_string())?;
        snapshot.flush().map_err(|e| e.to_string())?;
        let mut report = self.inspect(snapshot.path(), bytes.len() as u64)?;
        report["sha256"] = hex::encode(Sha256::digest(&bytes)).into();
        Ok((bytes, report))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn contract() -> AudioInputContract {
        AudioInputContract {
            sample_rates: vec![48_000],
            channel_order: vec!["left".into(), "right".into()],
            sample_formats: vec!["float32".into(), "pcm24".into()],
            max_seconds: 300,
            max_bytes: 128 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
            max_inputs: 2,
            simultaneous_comparison: true,
        }
    }

    pub(crate) fn stereo(path: &Path, frames: u32, first: [f32; 2], last: [f32; 2]) {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::File::create(path).unwrap();
        let data = frames * 8;
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + data).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&3u16.to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        file.write_all(&384_000u32.to_le_bytes()).unwrap();
        file.write_all(&8u16.to_le_bytes()).unwrap();
        file.write_all(&32u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data.to_le_bytes()).unwrap();
        for sample in first {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
        file.seek(SeekFrom::Start(44 + u64::from(data) - 8))
            .unwrap();
        for sample in last {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
        file.flush().unwrap();
    }

    #[test]
    fn agreement_preserves_antiphase_overrange_and_rejects_mismatch() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stereo.wav");
        stereo(&path, 4800, [2.0, -2.0], [0.25, -0.75]);
        let original = std::fs::read(&path).unwrap();
        let (bytes, report) = contract().read(&path, 256 * 1024 * 1024).unwrap();
        assert_eq!(bytes, original);
        assert_eq!(report["frames"], 4800);
        assert_eq!(report["gain_db"], 0.0);
        let mut incompatible = contract();
        incompatible.channel_order = vec!["mono".into()];
        assert!(
            incompatible
                .read(&path, 256 * 1024 * 1024)
                .unwrap_err()
                .contains("no resampling")
        );
        incompatible = contract();
        incompatible.max_bytes = 20;
        assert!(incompatible.read(&path, 256 * 1024 * 1024).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn malformed_and_over_duration_waveforms_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bad.wav");
        stereo(&path, 4800, [f32::NAN, 1.0], [0.0, 0.0]);
        assert!(
            contract()
                .read(&path, 256 * 1024 * 1024)
                .unwrap_err()
                .contains("non-finite")
        );
        stereo(&path, 48_000 * 300 + 1, [1.0, -1.0], [1.0, -1.0]);
        assert!(
            contract()
                .inspect(&path, std::fs::metadata(&path).unwrap().len())
                .unwrap_err()
                .contains("duration")
        );
    }
}
