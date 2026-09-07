//! Optional local MuScriptor subprocess, isolated from the desktop and its model licenses.

use crate::{AnalysisControl, SessionError};
use auris_core::AudioBuffer;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// Notice required before every MuScriptor invocation, including model-facing tools.
pub const MUSCRIPTOR_NOTICE: &str = "MuScriptor model weights are licensed under CC BY-NC 4.0 for noncommercial use only. Do not use this model for commercial work without separate permission from its rights holders. This optional model does not change Auris Studio's Apache-2.0 license. Acknowledgement does not grant commercial rights.";

/// Explicit per-run opt-in and local runtime files. Nothing is auto-installed or downloaded.
#[derive(Clone, Debug)]
pub struct MixtureOptions {
    /// Python executable in an environment containing muscriptor==0.3.0.
    pub python: PathBuf,
    /// Local MuScriptor Small safetensors file, with its original companion config.json.
    pub model: PathBuf,
    /// The caller has presented the notice and the user chose noncommercial use for this run.
    pub acknowledge_noncommercial: bool,
}

/// One note hypothesis with the model's original instrument-group name.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MixtureNote {
    /// MIDI pitch; drums retain their percussion pitch numbers.
    pub pitch: u8,
    /// Unquantized start relative to the analyzed source, in seconds.
    pub start: f64,
    /// Unquantized exclusive end, in seconds.
    pub end: f64,
    /// Original model group, including `drums`; it is a hypothesis, not a recovered patch.
    pub instrument: String,
}

/// Multi-instrument transcription draft; no guarantee of complete or correct recovery.
#[derive(Clone, Debug, Serialize)]
pub struct MixtureAnalysis {
    /// Inference backend and pinned package version.
    pub algorithm: &'static str,
    /// Provenance and model-use restriction retained in exported JSON.
    pub model_license: &'static str,
    /// Exact local checkpoint hash.
    pub model_sha256: String,
    /// Source duration in seconds, before padding or stretch.
    pub seconds: f64,
    /// Instrument-labeled, potentially overlapping notes.
    pub notes: Vec<MixtureNote>,
}

fn failure(message: impl ToString) -> SessionError {
    SessionError::MusicAnalysis(message.to_string())
}

impl MixtureOptions {
    /// Refuses unacknowledged or unprepared requests before decoding audio or starting Python.
    pub fn validate(&self) -> Result<(), SessionError> {
        if !self.acknowledge_noncommercial {
            return Err(failure(MUSCRIPTOR_NOTICE));
        }
        if !self.python.is_absolute() || !self.python.is_file() {
            return Err(failure(
                "select an absolute local MuScriptor Python executable",
            ));
        }
        if !self.model.is_absolute()
            || !self.model.is_file()
            || !self
                .model
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("safetensors"))
            || self.model.metadata().map_err(failure)?.len() > 512 * 1024 * 1024
        {
            return Err(failure(
                "select a local MuScriptor Small .safetensors file (at most 512 MiB)",
            ));
        }
        Ok(())
    }
}

// Ensures every exit path reaps the worker before its temporary files are removed.
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Validates the bounded worker contract independently of the optional runtime.
pub(crate) fn validate_notes(notes: &mut [MixtureNote], seconds: f64) -> Result<(), SessionError> {
    if notes.len() > 200_000
        || notes.iter().any(|n| {
            n.pitch > 127
                || !n.start.is_finite()
                || !n.end.is_finite()
                || n.start < 0.0
                || n.end <= n.start
                || n.start >= seconds
                || n.end > seconds + 10.0
                || n.instrument.is_empty()
                || n.instrument.len() > 96
                || n.instrument.chars().any(char::is_control)
        })
    {
        return Err(failure("invalid or excessive MuScriptor note events"));
    }
    for note in notes.iter_mut() {
        note.end = note.end.min(seconds);
    }
    notes.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then(a.instrument.cmp(&b.instrument))
            .then(a.pitch.cmp(&b.pitch))
    });
    Ok(())
}

pub(crate) fn transcribe_buffer(
    audio: &AudioBuffer,
    options: &MixtureOptions,
    control: &AnalysisControl,
) -> Result<MixtureAnalysis, SessionError> {
    options.validate()?;
    if control.is_cancelled() {
        return Err(failure("cancelled"));
    }
    if audio.sample_rate() != 16000.0
        || audio.frame_count() == 0
        || audio.frame_count() > 9_600_000
        || !(1..=8).contains(&audio.channel_count())
        || audio.iter_channels().flatten().any(|n| !n.is_finite())
    {
        return Err(failure(
            "MuScriptor supports at most ten minutes of finite 16 kHz audio / eight channels",
        ));
    }
    let directory = tempfile::tempdir().map_err(failure)?;
    let pcm = directory.path().join("audio.f32");
    let mut output = std::io::BufWriter::new(std::fs::File::create(pcm).map_err(failure)?);
    for i in 0..audio.frame_count() {
        let sample = audio
            .iter_channels()
            .map(|c| c[i] / audio.channel_count() as f32)
            .sum::<f32>();
        output.write_all(&sample.to_le_bytes()).map_err(failure)?;
    }
    output.flush().map_err(failure)?;
    drop(output);
    let mut hasher = Sha256::new();
    std::io::copy(
        &mut std::fs::File::open(&options.model).map_err(failure)?,
        &mut hasher,
    )
    .map_err(failure)?;
    let hash = format!("{:x}", hasher.finalize());
    let log_path = directory.path().join("stderr.txt");
    let log = std::fs::File::create(&log_path).map_err(failure)?;
    let mut command = Command::new(&options.python);
    command
        .args([
            "-I",
            "-c",
            include_str!("muscriptor_worker.py"),
            "--acknowledge-noncommercial",
            "--model",
        ])
        .arg(&options.model)
        .arg("--directory")
        .arg(directory.path())
        .env("HF_HUB_OFFLINE", "1")
        .env("CUDA_VISIBLE_DEVICES", "-1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = Worker(command.spawn().map_err(failure)?);
    let started = Instant::now();
    loop {
        if control.is_cancelled() {
            return Err(failure("cancelled"));
        }
        if started.elapsed() > Duration::from_secs(1800) {
            return Err(failure("MuScriptor exceeded the 30-minute CPU time limit"));
        }
        if let Some(status) = child.0.try_wait().map_err(failure)? {
            if !status.success() {
                let detail = read_bounded(&log_path, 16_384).unwrap_or_default();
                return Err(failure(format!(
                    "MuScriptor worker failed: {}",
                    String::from_utf8_lossy(&detail)
                )));
            }
            break;
        }
        if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > 1024 * 1024) {
            return Err(failure(
                "MuScriptor worker exceeded the diagnostic log limit",
            ));
        }
        if let Ok(bytes) = read_bounded(&directory.path().join("progress.json"), 64)
            && let Ok(fraction) = serde_json::from_slice::<f32>(&bytes)
            && fraction.is_finite()
            && (0.0..=1.0).contains(&fraction)
        {
            control.report_progress(fraction).map_err(failure)?;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let data = read_bounded(&directory.path().join("result.json"), 32 * 1024 * 1024)?;
    let mut notes: Vec<MixtureNote> = serde_json::from_slice(&data).map_err(failure)?;
    let seconds = audio.frame_count() as f64 / 16000.0;
    validate_notes(&mut notes, seconds)?;
    control.report_progress(1.0).map_err(failure)?;
    Ok(MixtureAnalysis {
        algorithm: "muscriptor-0.3.0-cpu",
        model_license: "CC-BY-NC-4.0",
        model_sha256: hash,
        seconds,
        notes,
    })
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, SessionError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(failure)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() as u64 > limit {
        return Err(failure("worker output exceeded its size limit"));
    }
    Ok(bytes)
}

/// Decodes an audio file only after explicit model-use acknowledgement.
pub fn transcribe_mixture_file(
    path: &Path,
    options: &MixtureOptions,
    control: &AnalysisControl,
) -> Result<MixtureAnalysis, SessionError> {
    options.validate()?;
    if control.is_cancelled() {
        return Err(failure("cancelled"));
    }
    let audio = super::decode_audio(path, 16000.0)?;
    transcribe_buffer(&audio, options, control)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_consent_refuses_before_opening_audio_or_starting_python() {
        let options = MixtureOptions {
            python: "missing-python".into(),
            model: "missing-model".into(),
            acknowledge_noncommercial: false,
        };
        let error =
            transcribe_mixture_file(Path::new("missing-audio"), &options, &Default::default())
                .unwrap_err();
        assert!(error.to_string().contains("CC BY-NC 4.0"));
    }
    #[test]
    fn rejects_invalid_notes_and_clips_padded_tails() {
        let mut notes = vec![MixtureNote {
            pitch: 60,
            start: 0.1,
            end: 1.2,
            instrument: "piano".into(),
        }];
        validate_notes(&mut notes, 1.0).unwrap();
        assert_eq!(notes[0].end, 1.0);
        notes[0].start = f64::NAN;
        assert!(validate_notes(&mut notes, 1.0).is_err());
    }
}
