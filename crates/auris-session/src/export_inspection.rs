//! Measurements of the encoded WAV on disk, after export has finished.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use auris_dsp::integrated_lufs;
use auris_gpu::analysis::analyze_loudness_cpu;
use auris_io::decode_audio_file;
use sha2::{Digest, Sha256};

use crate::SessionError;

/// Measurements of the actual decoded WAV file, including its ending and identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportInspection {
    /// Sample rate stored in the WAV header.
    pub sample_rate: u32,
    /// Number of encoded audio channels.
    pub channels: usize,
    /// Encoded bits per sample.
    pub bit_depth: u16,
    /// Decoded duration in seconds.
    pub seconds: f64,
    /// Decoded frames per channel.
    pub frames: u64,
    /// Integrated loudness of the saved samples; `None` for silence.
    pub lufs: Option<f32>,
    /// Highest decoded sample in dBFS.
    pub peak_db: f32,
    /// Estimated reconstructed true peak in dBTP.
    pub true_peak_db: f32,
    /// Samples at full scale, a warning of possible integer clipping.
    pub full_scale_samples: u64,
    /// RMS of the final half second in dBFS.
    pub ending_rms_db: f32,
    /// SHA-256 of the exact file measured.
    pub sha256: String,
}

/// Opens and measures an encoded WAV file rather than rendering the current project again.
///
/// The decoder's whole-buffer allocation limit applies. Large files should be split before
/// inspection until streaming loudness analysis is available.
pub fn inspect_export(path: &Path) -> Result<ExportInspection, SessionError> {
    let reader = hound::WavReader::open(path).map_err(|error| {
        SessionError::MixNormalization(format!("cannot read WAV {}: {error}", path.display()))
    })?;
    let spec = reader.spec();
    drop(reader);
    let decoded = decode_audio_file(path)?;
    let buffer = &decoded.buffer;
    let loudness = analyze_loudness_cpu(buffer);
    let frames = buffer.frame_count();
    let end_frames = ((buffer.sample_rate() * 0.5) as usize).min(frames);
    let from = frames - end_frames;
    let mut squares = 0.0f64;
    let mut count = 0usize;
    let mut full_scale_samples = 0u64;
    let full_scale_threshold = if spec.sample_format == hound::SampleFormat::Int {
        let step = 1.0 / 2.0f64.powi(i32::from(spec.bits_per_sample) - 1);
        1.0 - 1.5 * step
    } else {
        1.0 - 1.0e-6
    };
    for channel in buffer.iter_channels() {
        for &sample in channel {
            if f64::from(sample.abs()) >= full_scale_threshold {
                full_scale_samples += 1;
            }
        }
        for &sample in &channel[from..] {
            squares += f64::from(sample) * f64::from(sample);
            count += 1;
        }
    }
    let ending_rms_db = if count == 0 || squares == 0.0 {
        f32::NEG_INFINITY
    } else {
        (20.0 * (squares / count as f64).sqrt().log10()) as f32
    };
    let mut hasher = Sha256::new();
    let mut file = BufReader::new(File::open(path).map_err(|error| {
        SessionError::MixNormalization(format!("cannot hash {}: {error}", path.display()))
    })?);
    let mut chunk = [0u8; 65_536];
    loop {
        let size = file.read(&mut chunk).map_err(|error| {
            SessionError::MixNormalization(format!("cannot hash {}: {error}", path.display()))
        })?;
        if size == 0 {
            break;
        }
        hasher.update(&chunk[..size]);
    }
    Ok(ExportInspection {
        sample_rate: spec.sample_rate,
        channels: decoded.channel_count,
        bit_depth: spec.bits_per_sample,
        seconds: decoded.duration_seconds(),
        frames: frames as u64,
        lufs: integrated_lufs(buffer),
        peak_db: loudness.peak_db(),
        true_peak_db: loudness.true_peak_db(),
        full_scale_samples,
        ending_rms_db,
        sha256: hex::encode(hasher.finalize()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_reads_encoded_samples_and_the_final_half_second() {
        let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(file.path(), spec).unwrap();
        for frame in 0..48_000 {
            let sample = if frame < 24_000 {
                (0.2 * (2.0 * std::f64::consts::PI * 440.0 * frame as f64 / 48_000.0).sin()
                    * i16::MAX as f64) as i16
            } else {
                0
            };
            writer.write_sample(sample).unwrap();
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let measured = inspect_export(file.path()).unwrap();
        assert_eq!(measured.sample_rate, 48_000);
        assert_eq!(measured.channels, 2);
        assert_eq!(measured.bit_depth, 16);
        assert_eq!(measured.frames, 48_000);
        assert_eq!(measured.full_scale_samples, 0);
        assert_eq!(measured.ending_rms_db, f32::NEG_INFINITY);
        assert!(measured.lufs.is_some());
        assert_eq!(measured.sha256.len(), 64);
    }

    #[test]
    fn inspection_reports_saturated_integer_output() {
        let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..48_000 {
            writer.write_sample(i16::MAX).unwrap();
        }
        writer.finalize().unwrap();
        let measured = inspect_export(file.path()).unwrap();
        assert_eq!(measured.full_scale_samples, 48_000);
        assert!(measured.ending_rms_db > -0.1);
    }
}
