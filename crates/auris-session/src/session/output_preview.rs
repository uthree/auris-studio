//! Auditioning finished renders without processing them through the project a second time.

use std::sync::Arc;

use auris_core::AudioBuffer;
use auris_engine::EngineCommand;

use super::Session;
use crate::SessionError;

/// Prepares a level-adjusted, peak-bounded audition buffer on a worker thread.
///
/// Comparisons use a common RMS target of -20 dBFS, reduced when necessary to keep sample peaks
/// below -1 dBFS. This only applies a uniform gain; it does not compress, equalize or change the
/// retained render. Resampling precedes gain calculation so conversion overshoots are included.
pub fn prepare_output_preview(
    audio: &AudioBuffer,
    sample_rate: f64,
) -> Result<Arc<AudioBuffer>, SessionError> {
    if !sample_rate.is_finite() || !(8_000.0..=192_000.0).contains(&sample_rate) {
        return Err(SessionError::OutputPreview(
            "invalid output sample rate".into(),
        ));
    }
    if !audio.sample_rate().is_finite()
        || !(8_000.0..=192_000.0).contains(&audio.sample_rate())
        || audio.frame_count() == 0
        || audio.channel_count() > 2
        || audio
            .iter_channels()
            .flatten()
            .any(|value| !value.is_finite())
    {
        return Err(SessionError::OutputPreview(
            "audition needs finite, nonempty mono or stereo PCM at a supported sample rate".into(),
        ));
    }
    let mut prepared = if audio.sample_rate() == sample_rate {
        audio.clone()
    } else {
        auris_io::resample_buffer(audio, sample_rate)?
    };
    let samples = prepared.frame_count() * prepared.channel_count();
    let energy: f64 = prepared
        .iter_channels()
        .flatten()
        .map(|value| f64::from(*value).powi(2))
        .sum();
    let rms = (energy / samples.max(1) as f64).sqrt();
    let peak = prepared.peak();
    if !rms.is_finite() || rms <= 1e-8 || !peak.is_finite() {
        return Err(SessionError::OutputPreview(
            "the excerpt is silent or invalid".into(),
        ));
    }
    let gain = (0.1 / rms).min(f64::from(10.0f32.powf(-1.0 / 20.0) / peak));
    prepared.apply_gain(gain as f32);
    Ok(Arc::new(prepared))
}

impl Session {
    /// Plays prepared PCM directly at the output, preserving the project and undo history.
    ///
    /// Prepare with [`prepare_output_preview`] on a worker using [`Self::sample_rate`].
    /// An output-device change during preparation is rejected; prepare again at its new rate.
    /// Playback stops and effect tails clear before audition, without advancing the playhead.
    pub fn play_output_preview(&mut self, audio: Arc<AudioBuffer>) -> Result<(), SessionError> {
        if audio.sample_rate() != self.sample_rate() || audio.frame_count() == 0 {
            return Err(SessionError::OutputPreview(
                "the output rate changed; prepare the audition again".into(),
            ));
        }
        self.engine.send(EngineCommand::PlayOutputPreview(audio))?;
        Ok(())
    }

    /// Stops finished-audio audition while keeping the document and playhead untouched.
    pub fn stop_output_preview(&mut self) {
        self.send(EngineCommand::StopOutputPreview);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;

    fn tone(gain: f32) -> AudioBuffer {
        AudioBuffer::from_planar(
            vec![
                (0..4_800)
                    .map(|n| gain * (n as f32 * std::f32::consts::TAU / 48.0).sin())
                    .collect(),
            ],
            48_000.0,
        )
        .unwrap()
    }

    #[test]
    fn audition_matches_levels_and_preserves_the_retained_render() {
        let audio = tone(0.2);
        let original = audio.clone();
        let louder = prepare_output_preview(&tone(0.8), 48_000.0).unwrap();
        let quiet = prepare_output_preview(&audio, 48_000.0).unwrap();
        assert_eq!(audio, original);
        for (a, b) in louder.channel(0).iter().zip(quiet.channel(0)) {
            assert!((a - b).abs() < 1e-6);
        }
        assert!(quiet.peak() < 0.9);
    }

    #[test]
    fn conversion_and_playback_do_not_edit_the_document() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let original = session.project().clone();
        let revision = session.revision();
        let prepared = prepare_output_preview(&tone(0.4), session.sample_rate()).unwrap();
        session.play_output_preview(prepared).unwrap();
        session.stop_output_preview();
        assert_eq!(session.project(), &original);
        assert_eq!(session.revision(), revision);
        assert!(!session.can_undo());
        let converted = prepare_output_preview(&tone(0.4), 44_100.0).unwrap();
        assert_eq!(converted.sample_rate(), 44_100.0);
        assert!(session.play_output_preview(converted).is_err());
    }

    #[test]
    fn silent_nonfinite_and_unsafe_peaks_are_handled_before_the_callback() {
        assert!(prepare_output_preview(&AudioBuffer::stereo(4800, 48_000.0), 48_000.0).is_err());
        let mut audio = tone(0.4);
        audio.channel_mut(0)[0] = f32::NAN;
        assert!(prepare_output_preview(&audio, 48_000.0).is_err());
        audio.channel_mut(0)[0] = 100.0;
        let prepared = prepare_output_preview(&audio, 48_000.0).unwrap();
        assert!(prepared.peak() <= 0.892);
    }
}
