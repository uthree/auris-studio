//! Session boundary for optional noncommercial ONNX transcription.

use crate::{AnalysisControl, SessionError};
use auris_core::AudioBuffer;
use std::path::{Path, PathBuf};

pub use auris_analysis::mixture::{MUSCRIPTOR_NOTICE, MixtureAnalysis, MixtureNote};

/// Explicit per-run opt-in and the user's converted local model. No runtime preparation.
#[derive(Clone, Debug)]
pub struct MixtureOptions {
    /// Converted decoder.onnx beside audio.onnx and muscriptor.json.
    pub model: PathBuf,
    /// The caller presented the notice and the user chose noncommercial use for this run.
    pub acknowledge_noncommercial: bool,
}

fn failure(message: impl ToString) -> SessionError {
    SessionError::MusicAnalysis(message.to_string())
}

impl MixtureOptions {
    /// Refuses unacknowledged requests before decoding audio or reading a model.
    pub fn validate(&self) -> Result<(), SessionError> {
        if !self.acknowledge_noncommercial {
            return Err(failure(MUSCRIPTOR_NOTICE));
        }
        if self.model.file_name().and_then(|p| p.to_str()) != Some("decoder.onnx")
            || !self.model.is_file()
        {
            return Err(failure(
                "select decoder.onnx prepared by tools/music-models/export_muscriptor.py",
            ));
        }
        Ok(())
    }
}

pub(crate) fn validate_notes(notes: &mut [MixtureNote], seconds: f64) -> Result<(), SessionError> {
    auris_analysis::mixture::validate_notes(notes, seconds).map_err(failure)
}

pub(crate) fn transcribe_buffer(
    audio: &AudioBuffer,
    options: &MixtureOptions,
    control: &AnalysisControl,
) -> Result<MixtureAnalysis, SessionError> {
    options.validate()?;
    auris_analysis::mixture::transcribe(
        audio,
        &options.model,
        options.acknowledge_noncommercial,
        control,
    )
    .map_err(failure)
}

/// Decodes an audio file only after explicit noncommercial acknowledgement, then runs ONNX.
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
    #[ignore = "requires user-converted AURIS_MUSCRIPTOR_ONNX, AURIS_MUSCRIPTOR_AUDIO and AURIS_MUSCRIPTOR_REFERENCE"]
    fn onnx_matches_the_official_multi_chunk_note_reference() {
        let model = std::env::var_os("AURIS_MUSCRIPTOR_ONNX").expect("set converted model path");
        let audio =
            std::env::var_os("AURIS_MUSCRIPTOR_AUDIO").expect("set authorized fixture audio");
        let reference =
            std::env::var_os("AURIS_MUSCRIPTOR_REFERENCE").expect("set official JSON report");
        let expected: serde_json::Value =
            serde_json::from_slice(&std::fs::read(reference).unwrap()).unwrap();
        let notes: Vec<MixtureNote> = serde_json::from_value(expected["notes"].clone()).unwrap();
        assert!(!notes.is_empty(), "use a nonempty multi-chunk reference");
        let result = transcribe_mixture_file(
            Path::new(&audio),
            &MixtureOptions {
                model: model.into(),
                acknowledge_noncommercial: true,
            },
            &AnalysisControl::default(),
        )
        .unwrap();
        assert!(result.seconds > 5.0);
        assert_eq!(result.notes, notes);
    }
    #[test]
    fn no_consent_refuses_before_opening_audio_or_model() {
        let options = MixtureOptions {
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
