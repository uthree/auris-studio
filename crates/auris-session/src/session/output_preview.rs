//! Auditioning finished renders without processing them through the project a second time.

use std::sync::Arc;

use auris_core::AudioBuffer;
use auris_engine::{EngineCommand, OutputPreviewStatus};

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
    prepare_looping_output_preview(audio, sample_rate, None)
}

/// [`prepare_output_preview`] for audio that wraps to `loop_start_seconds` at its end.
///
/// A sample-rate converter reads either side of every frame, and at a buffer's edges it reads
/// silence; played on a loop, that dip lands on every wrap. The cycle is therefore converted
/// with its own frames as context on both sides — its start on the right of its end and, when
/// the loop starts at the top, its end on the left of its start — so the wrap is as seamless at
/// the device's rate as it was in the render. `None` is exactly [`prepare_output_preview`].
pub fn prepare_looping_output_preview(
    audio: &AudioBuffer,
    sample_rate: f64,
    loop_start_seconds: Option<f64>,
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
    let loop_start = loop_start_seconds
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * audio.sample_rate()).round() as usize)
        .filter(|start| *start < audio.frame_count());
    let mut prepared = if audio.sample_rate() == sample_rate {
        audio.clone()
    } else if let Some(start) = loop_start {
        resample_cycle(audio, sample_rate, start)?
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

/// Converts a buffer whose frames from `loop_start` on repeat, padding the cycle with itself.
fn resample_cycle(
    audio: &AudioBuffer,
    sample_rate: f64,
    loop_start: usize,
) -> Result<AudioBuffer, SessionError> {
    let source = audio.sample_rate().round() as u64;
    let target = sample_rate.round() as u64;
    let divisor = gcd(source, target);
    let (step_in, step_out) = (source / divisor, target / divisor);
    // A whole number of conversion steps, so the padding converts to a whole number of frames
    // and the cycle can be cut back out at an exact frame. Several resampler chunks wide.
    let pad = 8_192_u64.div_ceil(step_in) * step_in;
    let pad_out = usize::try_from(pad / step_in * step_out)
        .map_err(|_| SessionError::OutputPreview("audition loop padding overflows".into()))?;
    let pad = pad as usize;
    let frames = audio.frame_count();
    let cycle = frames - loop_start;
    let lead = if loop_start == 0 { pad } else { 0 };
    let channels = audio
        .channels()
        .iter()
        .map(|channel| {
            let cyclic = |i: usize| channel[loop_start + i % cycle];
            (0..lead)
                .map(|i| cyclic(i + cycle - lead % cycle))
                .chain(channel.iter().copied())
                .chain((0..pad).map(cyclic))
                .collect()
        })
        .collect();
    let padded = AudioBuffer::from_planar(channels, audio.sample_rate())?;
    let converted = auris_io::resample_buffer(&padded, sample_rate)?;
    let skip = if lead > 0 { pad_out } else { 0 };
    let length =
        usize::try_from((frames as u128 * u128::from(target)).div_ceil(u128::from(source)))
            .map_err(|_| SessionError::OutputPreview("audition is too long".into()))?;
    let channels = converted
        .channels()
        .iter()
        .map(|channel| channel[skip..skip + length].to_vec())
        .collect();
    Ok(AudioBuffer::from_planar(channels, sample_rate)?)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

impl Session {
    /// Plays prepared PCM directly at the output, preserving the project and undo history.
    ///
    /// Prepare with [`prepare_output_preview`] on a worker using [`Self::sample_rate`].
    /// An output-device change during preparation is rejected; prepare again at its new rate.
    /// Playback stops and effect tails clear before audition, without advancing the playhead.
    /// The returned status remains active while queued or playing, and clears on completion,
    /// cancellation, replacement or output-device loss. A headless session reports it inactive.
    /// Its [`OutputPreviewStatus::stop`] cancels this request even when the command queue is full.
    pub fn play_output_preview(
        &mut self,
        audio: Arc<AudioBuffer>,
    ) -> Result<OutputPreviewStatus, SessionError> {
        self.play_output_preview_with(audio, auris_engine::OutputPreviewOptions::default())
    }

    /// Plays prepared PCM with position and volume configured before the first audio block.
    pub fn play_output_preview_with(
        &mut self,
        audio: Arc<AudioBuffer>,
        options: auris_engine::OutputPreviewOptions,
    ) -> Result<OutputPreviewStatus, SessionError> {
        if audio.sample_rate() != self.sample_rate() || audio.frame_count() == 0 {
            return Err(SessionError::OutputPreview(
                "the output rate changed; prepare the audition again".into(),
            ));
        }
        Ok(self.engine.play_output_preview_with(audio, options)?)
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
    fn a_loop_converts_to_the_device_rate_without_a_dip_at_the_wrap() {
        // 50 cycles of 50 Hz in exactly one second, so the buffer is a seamless loop. A cosine,
        // because it starts and ends at its peak, where an edge effect has the most to take away.
        let tone = AudioBuffer::from_planar(
            vec![
                (0..22_050)
                    .map(|n| 0.5 * (n as f32 * std::f32::consts::TAU * 50.0 / 22_050.0).cos())
                    .collect(),
            ],
            22_050.0,
        )
        .unwrap();
        // Steepest step a 50 Hz cosine at 0.5 takes between two frames at 48 kHz, after gain.
        let wrap = |audio: &AudioBuffer| {
            let channel = audio.channel(0);
            let step = channel
                .windows(2)
                .skip(4_800)
                .take(4_800)
                .map(|pair| (pair[1] - pair[0]).abs())
                .fold(0.0, f32::max);
            ((channel[0] - channel[channel.len() - 1]).abs(), step)
        };
        let looped = prepare_looping_output_preview(&tone, 48_000.0, Some(0.0)).unwrap();
        assert_eq!(looped.frame_count(), 48_000);
        let (jump, step) = wrap(&looped);
        assert!(jump <= step * 1.05, "wrap {jump} against a step of {step}");
        // The ordinary conversion fades both edges towards silence, so its wrap is a click.
        let once = prepare_output_preview(&tone, 48_000.0).unwrap();
        let (jump, step) = wrap(&once);
        assert!(
            jump > step * 2.0,
            "the edge effect this avoids: {jump} against {step}"
        );
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
        let status = session.play_output_preview(prepared).unwrap();
        assert!(
            !status.is_active(),
            "a headless session has no audible playback"
        );
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
