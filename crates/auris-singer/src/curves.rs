//! Backend-independent curve prediction and musical expression.

use auris_vocal::{SingerFrames, SingerScore};

use crate::{SingError, validate_frames};
pub use auris_vocal::{CurveSource, CurveSources};

/// Predicts acoustic curves while retaining the backend's pronunciation or model context.
///
/// Implement this alongside [`crate::SingingBackend`] for a model that predicts pitch,
/// energy, or both. Call [`Self::prepare_curves`] during synthesis, then feed its resolved
/// curves and context to the decoder. No transport or tensor type crosses this interface.
pub trait CurveGenerator {
    /// Backend-owned information needed to decode the prediction, such as phoneme timing.
    type Context;

    /// Which curves this generator predicts; also used to sample the host's musical edits.
    const SOURCES: CurveSources;

    /// Sources for this predictor instance; optional model components may override defaults.
    fn curve_sources(&self) -> CurveSources {
        Self::SOURCES
    }

    /// Predicts unedited curves on the input frame clock, including declared context frames.
    ///
    /// `frames` supplies phonemes and musical controls sampled with [`Self::curve_sources`].
    /// Predict the base performance from the score; edits are applied by `prepare_curves`.
    /// `speaker` and `seed` have the same meanings as in the waveform synthesis contract.
    fn generate_curves(
        &mut self,
        frames: &SingerFrames,
        score: &SingerScore,
        speaker: u32,
        seed: u64,
    ) -> Result<CurvePrediction<Self::Context>, SingError>;

    /// Validates the input, predicts curves, and applies musical edits exactly once.
    fn prepare_curves(
        &mut self,
        frames: &SingerFrames,
        score: &SingerScore,
        speaker: u32,
        seed: u64,
    ) -> Result<PreparedCurves<Self::Context>, SingError> {
        validate_input(frames, score)?;
        let sources = self.curve_sources();
        self.generate_curves(frames, score, speaker, seed)?
            .apply_expression(frames, score, sources)
    }
}

/// Unedited predictions and decoder context on a shared frame grid.
pub struct CurvePrediction<C> {
    /// Predicted Hz, with zero preserving unvoiced frames; `None` for host-generated pitch.
    pub pitch_hz: Option<Vec<f64>>,
    /// Predicted nonnegative energy; `None` for host-generated energy.
    pub energy: Option<Vec<f64>>,
    /// Extra frames before the host timeline, included in each predicted array.
    pub leading_frames: usize,
    /// Extra frames after the host timeline, included in each predicted array.
    pub trailing_frames: usize,
    /// Pronunciation, tensors, or other data retained for the backend's decoder.
    pub context: C,
}

/// Acoustic curves ready for decoding, with musical edits already applied.
pub struct PreparedCurves<C> {
    /// Resolved pitch in Hz, including context frames.
    pub pitch_hz: Vec<f64>,
    /// Resolved energy, including context frames.
    pub energy: Vec<f64>,
    /// Frames to remove from the start of the decoded audio.
    pub leading_frames: usize,
    /// Frames to remove from the end of the decoded audio.
    pub trailing_frames: usize,
    /// The original backend context, unchanged by expression processing.
    pub context: C,
}

fn invalid(reason: &str) -> SingError {
    SingError::Inference(format!("singing curves: {reason}"))
}

fn validate_input(frames: &SingerFrames, score: &SingerScore) -> Result<(), SingError> {
    validate_frames(frames)?;
    if !frames.hop_seconds.is_finite() || frames.hop_seconds <= 0.0 {
        return Err(invalid("the frame clock must be positive and finite"));
    }
    if frames
        .f0_hz
        .iter()
        .chain(&frames.energy)
        .any(|v| !v.is_finite() || *v < 0.0)
    {
        return Err(invalid("host controls must be finite and nonnegative"));
    }
    let mut count = 0usize;
    for note in &score.notes {
        if note.frame_length == 0 || note.key.is_some_and(|key| key > 127) {
            return Err(invalid(
                "score events need positive lengths and valid MIDI keys",
            ));
        }
        count = count
            .checked_add(note.frame_length as usize)
            .ok_or_else(|| invalid("the score is too long"))?;
    }
    if count != frames.len() {
        return Err(invalid(
            "the score and host controls have different lengths",
        ));
    }
    Ok(())
}

impl<C> CurvePrediction<C> {
    /// Resolves host curves and predicted curves using the same musical-edit contract.
    ///
    /// Predicted pitch is multiplied by the host pitch / score-key frequency, preserving
    /// natural variation and voicing. Predicted energy is multiplied by velocity/expression.
    /// Controls extend backwards across rests for anticipated consonants and forwards into
    /// trailing context for releases. Predicted energy remains responsible for silence.
    /// Host-generated curves are copied verbatim, with silence in context frames.
    pub fn apply_expression(
        self,
        frames: &SingerFrames,
        score: &SingerScore,
        sources: CurveSources,
    ) -> Result<PreparedCurves<C>, SingError> {
        validate_input(frames, score)?;
        let count = frames
            .len()
            .checked_add(self.leading_frames)
            .and_then(|n| n.checked_add(self.trailing_frames))
            .ok_or_else(|| invalid("context length overflows the frame grid"))?;
        let resolve = |prediction: Option<Vec<f64>>, host: &[f32], source: CurveSource| match (
            source, prediction,
        ) {
            (CurveSource::Backend, Some(values)) => {
                if values.len() != count || values.iter().any(|v| !v.is_finite() || *v < 0.0) {
                    Err(invalid(
                        "predictions need one finite nonnegative value per frame",
                    ))
                } else {
                    Ok(values)
                }
            }
            (CurveSource::Host, None) => {
                let mut values = vec![0.0; count];
                for (dest, value) in values[self.leading_frames..][..frames.len()]
                    .iter_mut()
                    .zip(host)
                {
                    *dest = f64::from(*value);
                }
                Ok(values)
            }
            _ => Err(invalid(
                "predicted curves disagree with the declared sources",
            )),
        };
        let mut pitch = resolve(self.pitch_hz, &frames.f0_hz, sources.pitch)?;
        let mut energy = resolve(self.energy, &frames.energy, sources.energy)?;
        let mut controls = vec![None; count];
        let mut offset = self.leading_frames;
        for note in &score.notes {
            let end = offset + note.frame_length as usize;
            if let Some(key) = note.key {
                let base = auris_core::plugin::pitch_to_hz(f32::from(key));
                for (index, control) in controls[offset..end].iter_mut().enumerate() {
                    let source = offset + index - self.leading_frames;
                    let ratio = if frames.f0_hz[source] > 0.0 {
                        f64::from(frames.f0_hz[source] / base)
                    } else {
                        1.0
                    };
                    *control = Some((ratio, f64::from(frames.energy[source])));
                }
            }
            offset = end;
        }
        let mut next = None;
        for control in controls.iter_mut().rev() {
            if control.is_some() {
                next = *control;
            } else {
                *control = next;
            }
        }
        let mut previous = (1.0, 0.0);
        for (index, control) in controls.into_iter().enumerate() {
            let (ratio, gain) = control.unwrap_or(previous);
            previous = (ratio, gain);
            if sources.pitch == CurveSource::Backend {
                pitch[index] *= ratio;
            }
            if sources.energy == CurveSource::Backend {
                energy[index] *= gain;
            }
        }
        if pitch.iter().chain(&energy).any(|v| !v.is_finite()) {
            return Err(invalid("expression produced nonfinite acoustic values"));
        }
        Ok(PreparedCurves {
            pitch_hz: pitch,
            energy,
            leading_frames: self.leading_frames,
            trailing_frames: self.trailing_frames,
            context: self.context,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_vocal::SingerNote;

    /// A second generator with an ordinary Rust context, no VOICEVOX or JSON dependency.
    struct Predictor<const PITCH: bool, const ENERGY: bool>;

    impl<const PITCH: bool, const ENERGY: bool> CurveGenerator for Predictor<PITCH, ENERGY> {
        type Context = (u32, u64);
        const SOURCES: CurveSources = CurveSources {
            pitch: if PITCH {
                CurveSource::Backend
            } else {
                CurveSource::Host
            },
            energy: if ENERGY {
                CurveSource::Backend
            } else {
                CurveSource::Host
            },
        };
        fn generate_curves(
            &mut self,
            _: &SingerFrames,
            _: &SingerScore,
            speaker: u32,
            seed: u64,
        ) -> Result<CurvePrediction<Self::Context>, SingError> {
            Ok(CurvePrediction {
                pitch_hz: PITCH.then(|| vec![0.0, 0.0, 442.0, 438.0, 441.0, 0.0, 0.0, 0.0]),
                energy: ENERGY.then(|| vec![0.0, 0.04, 0.4, 0.2, 0.3, 0.0, 0.0, 0.0]),
                leading_frames: 1,
                trailing_frames: 2,
                context: (speaker, seed),
            })
        }
    }

    fn input() -> (SingerFrames, SingerScore) {
        let frames = SingerFrames {
            hop_seconds: 0.01,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![0, 1, 1, 1, 0],
            f0_hz: vec![0.0, 880.0, 440.0, 440.0, 0.0],
            energy: vec![0.0, 0.5, 0.25, 0.0, 0.0],
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
                    frame_length: 3,
                    lyric: "カ".into(),
                },
                SingerNote {
                    key: None,
                    frame_length: 1,
                    lyric: String::new(),
                },
            ],
        };
        (frames, score)
    }

    fn check_sources<const PITCH: bool, const ENERGY: bool>() {
        let (frames, score) = input();
        let prepared = Predictor::<PITCH, ENERGY>
            .prepare_curves(&frames, &score, 3, 42)
            .unwrap();
        assert_eq!(prepared.context, (3, 42));
        assert_eq!((prepared.leading_frames, prepared.trailing_frames), (1, 2));
        assert_eq!(
            prepared.pitch_hz,
            if PITCH {
                vec![0.0, 0.0, 884.0, 438.0, 441.0, 0.0, 0.0, 0.0]
            } else {
                vec![0.0, 0.0, 880.0, 440.0, 440.0, 0.0, 0.0, 0.0]
            }
        );
        assert_eq!(
            prepared.energy,
            if ENERGY {
                vec![0.0, 0.02, 0.2, 0.05, 0.0, 0.0, 0.0, 0.0]
            } else {
                vec![0.0, 0.0, 0.5, 0.25, 0.0, 0.0, 0.0, 0.0]
            }
        );
    }

    #[test]
    fn independent_sources_preserve_voicing_expression_and_decoder_context() {
        check_sources::<true, true>();
        check_sources::<true, false>();
        check_sources::<false, true>();
        check_sources::<false, false>();
    }

    #[test]
    fn instance_sources_override_defaults_when_preparing_predictions() {
        struct OptionalPredictor;
        impl CurveGenerator for OptionalPredictor {
            type Context = (u32, u64);
            const SOURCES: CurveSources = Predictor::<false, false>::SOURCES;
            fn curve_sources(&self) -> CurveSources {
                Predictor::<true, false>::SOURCES
            }
            fn generate_curves(
                &mut self,
                frames: &SingerFrames,
                score: &SingerScore,
                speaker: u32,
                seed: u64,
            ) -> Result<CurvePrediction<Self::Context>, SingError> {
                Predictor::<true, false>.generate_curves(frames, score, speaker, seed)
            }
        }
        let (frames, score) = input();
        let prepared = OptionalPredictor
            .prepare_curves(&frames, &score, 4, 9)
            .unwrap();
        assert_eq!(prepared.pitch_hz[2], 884.0);
        assert_eq!(prepared.energy[2], 0.5);
        assert_eq!(prepared.context, (4, 9));
    }

    #[test]
    fn malformed_predictions_and_mismatched_capabilities_are_errors() {
        let (frames, score) = input();
        for values in [
            vec![0.0],
            vec![-1.0; 8],
            vec![f64::NAN; 8],
            vec![f64::INFINITY; 8],
        ] {
            let mut prediction = Predictor::<true, true>
                .generate_curves(&frames, &score, 0, 0)
                .unwrap();
            prediction.pitch_hz = Some(values);
            assert!(
                prediction
                    .apply_expression(&frames, &score, Predictor::<true, true>::SOURCES)
                    .is_err()
            );
        }
        let prediction = Predictor::<true, false>
            .generate_curves(&frames, &score, 0, 0)
            .unwrap();
        assert!(
            prediction
                .apply_expression(&frames, &score, Predictor::<true, true>::SOURCES)
                .is_err()
        );
        let prediction = Predictor::<true, false>
            .generate_curves(&frames, &score, 0, 0)
            .unwrap();
        assert!(
            prediction
                .apply_expression(&frames, &score, CurveSources::default())
                .is_err()
        );
    }

    #[test]
    fn malformed_inputs_are_refused_before_prediction() {
        let (mut frames, mut score) = input();
        score.notes[1].frame_length += 1;
        assert!(
            Predictor::<true, true>
                .prepare_curves(&frames, &score, 0, 0)
                .is_err()
        );
        score.notes[1].frame_length -= 1;
        frames.f0_hz[1] = f32::NAN;
        assert!(
            Predictor::<true, true>
                .prepare_curves(&frames, &score, 0, 0)
                .is_err()
        );
        frames.f0_hz.pop();
        assert!(
            Predictor::<true, true>
                .prepare_curves(&frames, &score, 0, 0)
                .is_err()
        );
    }

    #[test]
    fn trailing_context_keeps_the_last_notes_controls() {
        let (mut frames, score) = input();
        frames.energy[3] = 0.5;
        let mut prediction = Predictor::<true, true>
            .generate_curves(&frames, &score, 0, 0)
            .unwrap();
        prediction.energy.as_mut().unwrap()[6] = 0.1;
        let prepared = prediction
            .apply_expression(&frames, &score, Predictor::<true, true>::SOURCES)
            .unwrap();
        assert_eq!(prepared.energy[6], 0.05);
    }
}
