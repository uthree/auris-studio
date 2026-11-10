//! Rendered excerpts for the controlling model to hear, independent of saving and transport.
//!
//! [`Session::begin_listening`] snapshots the open document; [`ListeningJob::run`] renders it on a
//! worker and encodes exact WAV bytes; [`Session::continue_listening`] accepts the result only for
//! the unchanged document. Nothing here talks to a model: the frontend that called `listen`
//! hands the clips to the model that asked, as audio input, and refuses the tool to a model that
//! cannot take audio.

use crate::{RenderJob, Session, prelude::*};
use auris_core::AudioBuffer;
use auris_gpu::analysis::analyze_loudness_cpu;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Retained raw excerpts are bounded to four stereo, 48 kHz, thirty-second buffers.
const RETAINED_EXCERPTS: usize = 4;

/// The rate every audition is rendered and encoded at.
pub const AUDITION_SAMPLE_RATE: u32 = 48_000;

#[derive(Default)]
/// Opaque bounded raw-audio history that a saved-project host may retain between calls.
pub struct ListeningContext {
    owner: Arc<()>,
    excerpts: VecDeque<Arc<Excerpt>>,
}

#[derive(Clone, PartialEq, Serialize)]
struct Range {
    start_bar: u32,
    bars: u32,
    start_seconds: f64,
    seconds: f64,
    timing_sha256: String,
}

struct Excerpt {
    id: String,
    revision: u64,
    range: Range,
    audio: AudioBuffer,
}

/// A render and optional immutable baseline, ready for a background worker.
pub struct ListeningJob {
    owner: Arc<()>,
    revision: u64,
    render: RenderJob,
    range: Range,
    previous: Option<Arc<Excerpt>>,
}

/// A finished render awaiting document/revision validation and bounded retention.
pub struct ListeningResult {
    owner: Arc<()>,
    excerpt: Arc<Excerpt>,
    audition: Audition,
}

/// One excerpt as the exact WAV bytes a model receives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditionClip {
    /// `excerpt` alone, or `before` and `after` for a comparison.
    pub label: String,
    /// Standard base64 of a 48 kHz, 16-bit PCM, stereo WAV file.
    pub wav_base64: String,
}

impl AuditionClip {
    /// The WAV bytes, decoded.
    pub fn wav(&self) -> Result<Vec<u8>, String> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(&self.wav_base64)
            .map_err(|error| error.to_string())
    }

    /// How long the clip plays, read from its WAV header without decoding the samples.
    pub fn seconds(&self) -> Result<f64, String> {
        let reader = hound::WavReader::new(std::io::Cursor::new(self.wav()?))
            .map_err(|error| error.to_string())?;
        let spec = reader.spec();
        Ok(f64::from(reader.duration()) / f64::from(spec.sample_rate.max(1)))
    }

    /// The clip as audio at `sample_rate`, for a person to hear what the model was played.
    pub fn audio(&self, sample_rate: f64) -> Result<AudioBuffer, String> {
        let mut reader = hound::WavReader::new(std::io::Cursor::new(self.wav()?))
            .map_err(|error| error.to_string())?;
        let spec = reader.spec();
        let count = usize::from(spec.channels.max(1));
        let scale = match spec.sample_format {
            hound::SampleFormat::Int => 1.0 / (1_i64 << (spec.bits_per_sample.max(1) - 1)) as f32,
            hound::SampleFormat::Float => 1.0,
        };
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => reader
                .samples::<i32>()
                .map(|sample| sample.map(|sample| sample as f32 * scale))
                .collect::<Result<_, _>>(),
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        }
        .map_err(|error| error.to_string())?;
        let mut channels = vec![Vec::with_capacity(samples.len() / count); count];
        for frame in samples.chunks_exact(count) {
            for (channel, sample) in channels.iter_mut().zip(frame) {
                channel.push(*sample);
            }
        }
        let audio = AudioBuffer::from_planar(channels, f64::from(spec.sample_rate))
            .map_err(|error| error.to_string())?;
        auris_io::resample_buffer(&audio, sample_rate).map_err(|error| error.to_string())
    }
}

/// What `listen` answers: a report for the model to read and the clips for it to hear.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Audition {
    /// IDs, bar range, revision and exact measurements of every clip, as JSON.
    pub report: serde_json::Value,
    /// Clips in listening order: `before` precedes `after` in a comparison.
    pub clips: Vec<AuditionClip>,
}

impl Audition {
    /// The report's text, which is the tool's text answer at every door.
    pub fn text(&self) -> String {
        self.report.to_string()
    }

    /// One line naming a clip, placed beside its audio so a listener knows which is which.
    pub fn caption(&self, clip: &AuditionClip) -> String {
        let range = &self.report["range"];
        format!(
            "Auris audition {} ({}): bars {}..{} of the open song, {:.2} seconds, rendered from revision {}.",
            self.report["audition_id"].as_str().unwrap_or_default(),
            clip.label,
            range["start_bar"],
            range["start_bar"].as_u64().unwrap_or(1) + range["bars"].as_u64().unwrap_or(1) - 1,
            range["seconds"].as_f64().unwrap_or_default(),
            self.report["clips"]
                .as_array()
                .and_then(|clips| clips.iter().find(|entry| entry["label"] == clip.label))
                .map_or(serde_json::Value::Null, |entry| entry["revision"].clone()),
        )
    }
}

/// Measurements of the source render and the actual, decoded audition WAV.
#[derive(Debug, Serialize)]
pub struct AuditionMeasurements {
    /// Sample peak before protecting the audition file, in dBFS (silence is -120).
    pub source_peak_dbfs: f32,
    /// Estimated true peak before audition protection; 4x FIR, not a certified BS.1770 meter.
    pub source_true_peak_estimate_dbfs: f32,
    /// Number of source samples at or above full scale; protection does not repair the mix.
    pub source_full_scale_samples: usize,
    /// Common gain used for the audition, never positive; the project stays unchanged.
    pub attenuation_db: f32,
    /// Sample peak measured after encoding and decoding the actual WAV.
    pub peak_dbfs: f32,
    /// Estimated true peak measured on the encoded WAV.
    pub true_peak_estimate_dbfs: f32,
    /// RMS measured on the encoded WAV, without independent loudness normalization.
    pub rms_dbfs: f32,
    /// SHA-256 of the exact WAV bytes delivered for this excerpt.
    pub wav_sha256: String,
    /// Rate of the rendered and encoded excerpt.
    pub sample_rate: f64,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Audio audition cancelled".into())
    } else {
        Ok(())
    }
}

fn protective_gain(audio: &AudioBuffer) -> Result<f32, String> {
    if audio
        .iter_channels()
        .flatten()
        .any(|sample| !sample.is_finite())
    {
        return Err("Audio audition contains non-finite samples".into());
    }
    // Measure the actual render rate. Instrument response can differ from a prior inspection.
    Ok(analyze_loudness_cpu(audio)
        .normalization_gain(-1.0)
        .min(1.0))
}

fn encode_audition(
    path: &Path,
    audio: &AudioBuffer,
    gain: f32,
    cancel: &AtomicBool,
) -> Result<(AuditionMeasurements, Vec<u8>), String> {
    use sha2::{Digest, Sha256};
    check_cancel(cancel)?;
    let source = analyze_loudness_cpu(audio);
    let full_scale = audio
        .iter_channels()
        .flatten()
        .filter(|sample| sample.abs() >= 1.0)
        .count();
    let mut protected = audio.clone();
    protected.apply_gain(gain);
    let settings = auris_io::WavExportSettings {
        bit_depth: auris_io::WavBitDepth::Int16,
        sample_rate: audio.sample_rate().round() as u32,
        ..Default::default()
    };
    auris_io::write_audio_with_progress(path, &protected, &settings.into(), &mut |_| {
        !cancel.load(Ordering::Relaxed)
    })
    .map_err(|error| error.to_string())?;
    check_cancel(cancel)?;
    let decoded =
        crate::decode_audio(path, audio.sample_rate()).map_err(|error| error.to_string())?;
    let measured = analyze_loudness_cpu(&decoded);
    if measured.peak >= 1.0 || measured.true_peak_db() > -0.99 {
        return Err("Encoded audition exceeds its peak ceiling; no audio was delivered".into());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    Ok((
        AuditionMeasurements {
            source_peak_dbfs: source.peak_db(),
            source_true_peak_estimate_dbfs: source.true_peak_db(),
            source_full_scale_samples: full_scale,
            attenuation_db: if gain < 1.0 { 20.0 * gain.log10() } else { 0.0 },
            peak_dbfs: measured.peak_db(),
            true_peak_estimate_dbfs: measured.true_peak_db(),
            rms_dbfs: measured.rms_db(),
            wav_sha256: hex::encode(Sha256::digest(&bytes)),
            sample_rate: decoded.sample_rate(),
        },
        bytes,
    ))
}

/// Writes a protected audition WAV and measures its exact encoded result.
///
/// Attenuates only when necessary to keep the estimated true peak at -1 dB. Does not
/// change the project, increase quiet audio, or normalize separate excerpts independently.
pub fn write_audition_wav(
    path: &Path,
    audio: &AudioBuffer,
    cancel: &AtomicBool,
) -> Result<AuditionMeasurements, String> {
    check_cancel(cancel)?;
    let gain = protective_gain(audio)?;
    encode_audition(path, audio, gain, cancel).map(|(measurements, _)| measurements)
}

/// Encodes one excerpt through a temporary file, which is removed before returning.
fn audition_clip(
    audio: &AudioBuffer,
    gain: f32,
    cancel: &AtomicBool,
) -> Result<(AuditionMeasurements, String), String> {
    use base64::Engine;
    let file = tempfile::Builder::new()
        .prefix("auris-audition-")
        .suffix(".wav")
        .tempfile()
        .map_err(|error| error.to_string())?
        .into_temp_path();
    let (measurements, bytes) = encode_audition(&file, audio, gain, cancel)?;
    Ok((
        measurements,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

impl Session {
    /// Transfer listening history without changing the document, paths or Undo state.
    pub fn swap_listening_context(&mut self, context: &mut ListeningContext) {
        std::mem::swap(&mut self.listening, context);
    }

    /// Snapshots the unsaved mix for the model to hear. Does not edit, save or add Undo.
    ///
    /// `compare_to` is an opaque ID from an earlier successful call in this document, and needs
    /// the same bars and timing. At most four raw excerpts are retained.
    pub fn begin_listening(
        &mut self,
        start_bar: u32,
        bars: u32,
        compare_to: Option<&str>,
    ) -> Result<ListeningJob, String> {
        let (start, end) = crate::audio_inspection::bar_range(self.project(), start_bar, bars)?;
        let start_seconds = self.project().tempo_map.ticks_to_seconds(start).0;
        let range = Range {
            start_bar,
            bars,
            start_seconds,
            seconds: self.project().tempo_map.ticks_to_seconds(end).0 - start_seconds,
            timing_sha256: {
                use sha2::{Digest, Sha256};
                let timing =
                    serde_json::to_vec(&(&self.project().tempo_map, &self.project().signatures))
                        .map_err(|error| error.to_string())?;
                hex::encode(Sha256::digest(timing))
            },
        };
        let previous = compare_to
            .map(|id| {
                self.listening
                    .excerpts
                    .iter()
                    .find(|entry| entry.id == id)
                    .cloned()
                    .ok_or("Unknown or expired audition_id; listen again for a new baseline")
            })
            .transpose()?;
        if previous.as_ref().is_some_and(|entry| entry.range != range) {
            return Err("Comparison requires the same bars and timing as the baseline; listen again for a new baseline after tempo or meter changes".into());
        }
        Ok(ListeningJob {
            owner: self.listening.owner.clone(),
            revision: self.revision(),
            render: self.render_job_at(Some(f64::from(AUDITION_SAMPLE_RATE))),
            range,
            previous,
        })
    }

    /// Accepts an audition only for the unchanged document; retains its raw audio for comparison.
    pub fn continue_listening(&mut self, result: ListeningResult) -> Result<Audition, String> {
        if !Arc::ptr_eq(&self.listening.owner, &result.owner)
            || self.revision() != result.excerpt.revision
        {
            return Err("The document changed during listening; listen again".into());
        }
        self.listening.excerpts.push_back(result.excerpt);
        while self.listening.excerpts.len() > RETAINED_EXCERPTS {
            self.listening.excerpts.pop_front();
        }
        Ok(result.audition)
    }
}

impl ListeningJob {
    /// Renders at 48 kHz and encodes exact WAV bytes; a comparison encodes the baseline too.
    ///
    /// Both files of a comparison share one protective gain, so their relative level is the
    /// mix's own; raw baselines never change. Cancellation is checked around rendering and
    /// encoding.
    pub fn run(mut self, cancel: &AtomicBool) -> Result<ListeningResult, String> {
        check_cancel(cancel)?;
        self.render
            .validate_complete()
            .map_err(|error| error.to_string())?;
        let rate = f64::from(AUDITION_SAMPLE_RATE);
        let options = OfflineOptions {
            start_frames: (self.range.start_seconds * rate).round() as u64,
            end_frames: Some(
                ((self.range.start_seconds + self.range.seconds) * rate).round() as u64,
            ),
            sample_rate: Some(rate),
            include_tail: false,
            ..Default::default()
        };
        let audio = self
            .render
            .render(
                &options,
                &mut RenderProgress::default().cancelled_by(cancel),
            )
            .map_err(|error| error.to_string())?;
        check_cancel(cancel)?;
        let mut gain = protective_gain(&audio)?;
        if let Some(previous) = &self.previous {
            gain = gain.min(protective_gain(&previous.audio)?);
        }
        let excerpt = Arc::new(Excerpt {
            id: crate::transient_id::transient_id("audition"),
            revision: self.revision,
            range: self.range,
            audio,
        });
        let mut clips = Vec::new();
        let mut entries = Vec::new();
        if let Some(previous) = &self.previous {
            let (measurements, wav) = audition_clip(&previous.audio, gain, cancel)?;
            entries.push(json!({"label":"before","audition_id":previous.id,
                "revision":previous.revision,"audio":measurements}));
            clips.push(AuditionClip {
                label: "before".into(),
                wav_base64: wav,
            });
        }
        let label = if self.previous.is_some() {
            "after"
        } else {
            "excerpt"
        };
        let (measurements, wav) = audition_clip(&excerpt.audio, gain, cancel)?;
        entries.push(json!({"label":label,"audition_id":excerpt.id,
            "revision":excerpt.revision,"audio":measurements}));
        clips.push(AuditionClip {
            label: label.into(),
            wav_base64: wav,
        });
        check_cancel(cancel)?;
        let report = json!({
            "audition_id":excerpt.id,"revision":excerpt.revision,"range":excerpt.range,
            "compare_to":self.previous.as_ref().map(|entry| &entry.id),
            "clips":entries,
            "representation":"48 kHz 16-bit PCM stereo WAV, delivered to you as audio input in the order of clips, each preceded by a caption naming it. A comparison shares one protective gain, so relative level is the mix's own.",
            "next_step":"Judge only what you actually hear in the delivered audio. If the audio did not reach you, say that nothing was heard. After an edit, listen to the same bars with compare_to set to this audition_id to hear before and after. The latest four audition_ids are retained."
        });
        Ok(ListeningResult {
            owner: self.owner,
            excerpt,
            audition: Audition { report, clips },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Session, TrackId) {
        let mut session =
            Session::new(crate::SessionOptions::headless().with_balance(false)).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .set_track_instrument(track, "auris.synth.fm2")
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(8.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::from_beats(3.0)))
            .unwrap();
        (session, track)
    }

    fn listen(session: &mut Session, previous: Option<&str>) -> Audition {
        let job = session.begin_listening(1, 1, previous).unwrap();
        let result = job.run(&AtomicBool::new(false)).unwrap();
        session.continue_listening(result).unwrap()
    }

    fn rms(bytes: &[u8]) -> f64 {
        let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, AUDITION_SAMPLE_RATE);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.bits_per_sample, 16);
        let samples: Vec<_> = reader.into_samples::<i16>().map(Result::unwrap).collect();
        assert!(
            samples
                .iter()
                .all(|sample| *sample != i16::MIN && *sample != i16::MAX)
        );
        (samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }

    #[test]
    fn a_clip_plays_back_at_the_window_rate_for_as_long_as_its_header_says() {
        let (mut session, _) = fixture();
        let audition = listen(&mut session, None);
        let clip = &audition.clips[0];
        let seconds = clip.seconds().unwrap();
        // One 4/4 bar at the default 120 BPM.
        assert!((seconds - 2.0).abs() < 0.01, "{seconds}");
        let native = clip.audio(f64::from(AUDITION_SAMPLE_RATE)).unwrap();
        assert_eq!(native.channel_count(), 2);
        assert_eq!(native.frame_count(), (seconds * 48_000.0).round() as usize);
        assert!(native.channel(0).iter().any(|sample| sample.abs() > 0.01));
        let window = clip.audio(44_100.0).unwrap();
        assert_eq!(window.sample_rate(), 44_100.0);
        assert!((window.duration_seconds() - seconds).abs() < 0.01);
    }

    #[test]
    fn unsaved_listen_edit_compare_returns_real_audio_and_keeps_the_baseline() {
        let (mut session, track) = fixture();
        let original = session.project().clone();
        let original_revision = session.revision();
        let first = listen(&mut session, None);
        assert_eq!(session.project(), &original);
        assert_eq!(session.revision(), original_revision);
        assert!(session.path().is_none());
        assert_eq!(first.clips.len(), 1);
        assert_eq!(first.clips[0].label, "excerpt");
        let first_wav = first.clips[0].wav().unwrap();
        let frames = hound::WavReader::new(std::io::Cursor::new(&first_wav))
            .unwrap()
            .duration();
        let seconds = first.report["range"]["seconds"].as_f64().unwrap();
        assert_eq!(frames, (seconds * 48_000.0).round() as u32);
        assert!(rms(&first_wav) > 0.0);

        session.set_param(crate::ParamTarget::TrackGain(track), -12.0);
        let second = listen(&mut session, first.report["audition_id"].as_str());
        assert_eq!(second.report["compare_to"], first.report["audition_id"]);
        let labels: Vec<_> = second
            .clips
            .iter()
            .map(|clip| clip.label.as_str())
            .collect();
        assert_eq!(labels, ["before", "after"]);
        assert_eq!(
            second.report["clips"][0]["revision"].as_u64(),
            Some(original_revision)
        );
        assert!(second.report["revision"].as_u64().unwrap() > original_revision);
        let before = second.clips[0].wav().unwrap();
        let after = second.clips[1].wav().unwrap();
        assert_eq!(before, first_wav, "the baseline is delivered unchanged");
        assert!((20.0 * (rms(&after) / rms(&before)).log10() + 12.0).abs() < 0.02);
        assert!(second.caption(&second.clips[0]).contains("(before)"));
        session.undo();
        assert_eq!(
            session.project(),
            &original,
            "listening must not add an undo checkpoint"
        );
    }

    #[test]
    fn overloaded_comparison_uses_one_protective_gain_for_both_excerpts() {
        let (mut session, track) = fixture();
        let first = listen(&mut session, None);
        let original = session.listening.excerpts[0].audio.clone();
        session.set_param(crate::ParamTarget::TrackGain(track), 30.0);
        let second = listen(&mut session, first.report["audition_id"].as_str());
        let after = &second.report["clips"][1]["audio"];
        let before = &second.report["clips"][0]["audio"];
        assert!(after["source_full_scale_samples"].as_u64().unwrap() > 0);
        assert!(after["attenuation_db"].as_f64().unwrap() < 0.0);
        assert_eq!(after["attenuation_db"], before["attenuation_db"]);
        assert!(after["true_peak_estimate_dbfs"].as_f64().unwrap() < -0.99);
        assert_eq!(
            session.listening.excerpts[0].audio.channels(),
            original.channels()
        );
        assert_eq!(session.project().track(track).unwrap().mixer.gain_db, 30.0);
        let pair: Vec<_> = second
            .clips
            .iter()
            .map(|clip| clip.wav().unwrap())
            .collect();
        assert!((20.0 * (rms(&pair[1]) / rms(&pair[0])).log10() - 30.0).abs() < 0.1);
    }

    #[test]
    fn stale_results_invalid_comparisons_and_cancellation_do_not_publish() {
        let (mut session, track) = fixture();
        let first = listen(&mut session, None);
        let baseline = first.report["audition_id"].as_str().unwrap();
        assert!(session.begin_listening(2, 1, Some(baseline)).is_err());
        assert!(
            session
                .begin_listening(1, 1, Some("C:/external.wav"))
                .is_err()
        );
        assert!(session.begin_listening(1, 9, None).is_err());
        session
            .agent_command(crate::live_agent::Command::EditTimeline {
                operation: crate::live_agent::document_editing::TimelineOperation::SetTempo {
                    at_tick: 0,
                    bpm: 90.0,
                },
            })
            .unwrap();
        assert!(session.begin_listening(1, 1, Some(baseline)).is_err());
        session.undo();
        let cancelled = session.begin_listening(1, 1, None).unwrap();
        assert!(cancelled.run(&AtomicBool::new(true)).is_err());
        let job = session.begin_listening(1, 1, Some(baseline)).unwrap();
        session.set_param(crate::ParamTarget::TrackGain(track), -3.0);
        let result = job.run(&AtomicBool::new(false)).unwrap();
        assert!(
            session
                .continue_listening(result)
                .unwrap_err()
                .contains("document changed")
        );
        assert_eq!(session.listening.excerpts.len(), 1);
        session.new_project();
        assert!(session.begin_listening(1, 1, Some(baseline)).is_err());
    }

    #[test]
    fn old_ids_are_evicted_and_other_sessions_cannot_accept_the_result() {
        let (mut session, _) = fixture();
        let first = listen(&mut session, None);
        for _ in 0..RETAINED_EXCERPTS {
            listen(&mut session, None);
        }
        assert_eq!(session.listening.excerpts.len(), RETAINED_EXCERPTS);
        assert!(
            session
                .begin_listening(1, 1, first.report["audition_id"].as_str())
                .is_err()
        );
        let result = session
            .begin_listening(1, 1, None)
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        let (mut other, _) = fixture();
        assert!(other.continue_listening(result).is_err());
    }

    #[test]
    fn nonfinite_audio_is_rejected_before_wav_encoding() {
        let audio = AudioBuffer::from_planar(vec![vec![f32::NAN]], 48000.0).unwrap();
        let target = tempfile::NamedTempFile::new().unwrap();
        assert!(write_audition_wav(target.path(), &audio, &AtomicBool::new(false)).is_err());
        assert_eq!(target.as_file().metadata().unwrap().len(), 0);
    }

    #[test]
    fn the_wire_form_round_trips() {
        let (mut session, _) = fixture();
        let audition = listen(&mut session, None);
        let wire = serde_json::to_string(&audition).unwrap();
        let back: Audition = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, audition);
        assert_eq!(back.text(), audition.report.to_string());
    }
}
