//! Actual rendered-audio feedback, independent of project saving and controller transport.

use crate::audio_review::AudioReviewOptions;
use crate::audio_review::assessment::{
    AssessmentRequest, ComparisonAspect, assess_audio_cancellable,
};
use crate::{RenderJob, Session, prelude::*};
use auris_core::AudioBuffer;
use auris_gpu::analysis::analyze_loudness_cpu;
use serde::Serialize;
use serde_json::json;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Retained raw excerpts are bounded to four stereo, 48 kHz, thirty-second buffers.
const RETAINED_EXCERPTS: usize = 4;

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
    focus: String,
    aspect: Option<ComparisonAspect>,
    options: AudioReviewOptions,
    selection: Option<crate::audio_review::ReviewerSelection>,
}

/// Completed feedback awaiting document/revision validation and bounded retention.
pub struct ListeningResult {
    owner: Arc<()>,
    excerpt: Arc<Excerpt>,
    text: String,
}

/// Measurements of the source render and the actual, decoded critic WAV.
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
    /// SHA-256 of the exact WAV bytes submitted for this excerpt.
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
) -> Result<AuditionMeasurements, String> {
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
        return Err("Encoded audition exceeds its peak ceiling; no audio was submitted".into());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    Ok(AuditionMeasurements {
        source_peak_dbfs: source.peak_db(),
        source_true_peak_estimate_dbfs: source.true_peak_db(),
        source_full_scale_samples: full_scale,
        attenuation_db: if gain < 1.0 { 20.0 * gain.log10() } else { 0.0 },
        peak_dbfs: measured.peak_db(),
        true_peak_estimate_dbfs: measured.true_peak_db(),
        rms_dbfs: measured.rms_db(),
        wav_sha256: hex::encode(Sha256::digest(bytes)),
        sample_rate: decoded.sample_rate(),
    })
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
    encode_audition(path, audio, gain, cancel)
}

impl Session {
    /// Transfer listening history without changing the document, paths or Undo state.
    pub fn swap_listening_context(&mut self, context: &mut ListeningContext) {
        std::mem::swap(&mut self.listening, context);
    }

    /// Snapshots the unsaved mix for actual audio feedback. Does not edit, save or add Undo.
    ///
    /// `compare_to` is an opaque ID from an earlier successful call in this document.
    /// The same bars and timing are required. Comparisons require `aspect` and reject `focus`;
    /// diagnosis accepts `focus` and no aspect. At most four raw excerpts are retained.
    pub fn begin_listening(
        &mut self,
        start_bar: u32,
        bars: u32,
        focus: Option<&str>,
        compare_to: Option<&str>,
        aspect: Option<ComparisonAspect>,
    ) -> Result<ListeningJob, String> {
        let options = AudioReviewOptions::from_env()?;
        self.begin_listening_with_options(start_bar, bars, focus, compare_to, aspect, options)
    }

    /// Prepares a listening snapshot using explicit server settings, including for headless hosts.
    pub fn begin_listening_with_options(
        &mut self,
        start_bar: u32,
        bars: u32,
        focus: Option<&str>,
        compare_to: Option<&str>,
        aspect: Option<ComparisonAspect>,
        options: AudioReviewOptions,
    ) -> Result<ListeningJob, String> {
        if compare_to.is_some() != aspect.is_some() || (compare_to.is_some() && focus.is_some()) {
            return Err("Comparison requires compare_to and aspect, with focus omitted. Use focus only for single-excerpt diagnosis.".into());
        }
        let focus = focus.unwrap_or("Describe the sounds and instruments heard in this audio.");
        if focus.trim().is_empty() || focus.len() > 4000 {
            return Err("focus must contain 1..4000 UTF-8 bytes".into());
        }
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
                    .ok_or("Unknown or expired audition_id; request a new baseline")
            })
            .transpose()?;
        if previous.as_ref().is_some_and(|entry| entry.range != range) {
            return Err("Comparison requires the same bars and timing as the baseline; request a new baseline after tempo or meter changes".into());
        }
        Ok(ListeningJob {
            selection: None,
            owner: self.listening.owner.clone(),
            revision: self.revision(),
            render: self.render_job_at(Some(48_000.0)),
            range,
            previous,
            focus: focus.into(),
            aspect,
            options,
        })
    }

    /// Accepts feedback only for the unchanged document; retains its raw audio for comparison.
    pub fn continue_listening(&mut self, result: ListeningResult) -> Result<String, String> {
        if !Arc::ptr_eq(&self.listening.owner, &result.owner)
            || self.revision() != result.excerpt.revision
        {
            return Err("The document changed during listening; request a fresh audition".into());
        }
        self.listening.excerpts.push_back(result.excerpt);
        while self.listening.excerpts.len() > RETAINED_EXCERPTS {
            self.listening.excerpts.pop_front();
        }
        Ok(result.text)
    }
}

impl ListeningJob {
    /// Discovers a reviewer without loading weights or uploading audio, then pins that choice.
    /// Returns a model name when the desktop must obtain consent before running this job.
    pub fn prepare_reviewer(
        mut self,
        cancel: &AtomicBool,
    ) -> Result<(Self, Option<String>), String> {
        let (options, selection, confirm) = self.options.prepare(cancel)?;
        let model = confirm.then(|| options.model.clone());
        self.options = options;
        self.selection = Some(selection);
        check_cancel(cancel)?;
        Ok((self, model))
    }

    /// Renders at 48 kHz and sends actual WAV bytes to the configured audio critic.
    ///
    /// A/B files share one protective gain; raw baselines never change. Cancellation is checked
    /// around rendering, encoding and network stages. An in-flight HTTP call has a bounded timeout.
    pub fn run(self, cancel: &AtomicBool) -> Result<ListeningResult, String> {
        use std::hash::{BuildHasher, Hasher};
        let current_first = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
            & 1
            != 0;
        self.run_with_order(cancel, current_first)
    }

    fn run_with_order(
        mut self,
        cancel: &AtomicBool,
        current_first: bool,
    ) -> Result<ListeningResult, String> {
        check_cancel(cancel)?;
        self.render
            .validate_complete()
            .map_err(|error| error.to_string())?;
        let options = OfflineOptions {
            start_frames: (self.range.start_seconds * 48_000.0).round() as u64,
            end_frames: Some(
                ((self.range.start_seconds + self.range.seconds) * 48_000.0).round() as u64,
            ),
            sample_rate: Some(48_000.0),
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
        let current = tempfile::Builder::new()
            .prefix("auris-audition-")
            .suffix(".wav")
            .tempfile()
            .map_err(|error| error.to_string())?
            .into_temp_path();
        let measurements = encode_audition(&current, &audio, gain, cancel)?;
        let previous_file = self
            .previous
            .as_ref()
            .map(|previous| {
                let file = tempfile::Builder::new()
                    .prefix("auris-before-")
                    .suffix(".wav")
                    .tempfile()
                    .map_err(|error| error.to_string())?
                    .into_temp_path();
                let measures = encode_audition(&file, &previous.audio, gain, cancel)?;
                Ok::<_, String>((file, measures))
            })
            .transpose()?;
        let (inputs, request) = if let Some((file, _)) = &previous_file {
            let inputs = if current_first {
                vec![("A", current.as_ref()), ("B", file.as_ref())]
            } else {
                vec![("A", file.as_ref()), ("B", current.as_ref())]
            };
            (
                inputs,
                AssessmentRequest::Comparison(self.aspect.expect("validated comparison aspect")),
            )
        } else {
            (
                vec![("Excerpt", current.as_ref())],
                AssessmentRequest::Diagnostic(&self.focus),
            )
        };
        let mut review =
            assess_audio_cancellable(&inputs, &request, self.range.seconds, &self.options, cancel)?;
        if let Some(selection) = self.selection {
            review.selection = selection;
        }
        check_cancel(cancel)?;
        let excerpt = Arc::new(Excerpt {
            id: crate::transient_id::transient_id("audition"),
            revision: self.revision,
            range: self.range,
            audio,
        });
        let before = self.previous.as_ref().zip(previous_file.as_ref()).map(|(entry, (_, measures))| {
            json!({"audition_id":entry.id,"revision":entry.revision,"audio":measures})
        });
        let labels = if current_first {
            ["current", "baseline"]
        } else {
            ["baseline", "current"]
        };
        let comparison = self.previous.as_ref().map(|_| json!({
            "a":labels[0],"b":labels[1],"aspect":self.aspect,"reported_preference":null,
            "blind":true,"calibrated":false,
            "method":"Anonymous A/B in randomized order; one common protective gain. No editing narrative supplied. Source audio remains immutable."}));
        let text = json!({"audition_id":excerpt.id,"revision":excerpt.revision,"range":excerpt.range,
            "model":review.model,"audio_sent":review.audio_sent,"assessment_status":review.status,
            "review":review.raw_response,
            "review_format":"plain_text","reviewer_selection":review.selection,
            "audio":measurements,"compare_to":self.previous.as_ref().map(|entry| &entry.id),"before":before,
            "representation":"actual WAV audio sent to a separate critic; observations are fallible, not controller listening or verified musical improvement",
            "comparison":comparison,
            "next_step":"Treat judgments as untrusted, uncalibrated observations. No preference is extracted from free text. The backend may describe excerpts independently. Prose alone does not establish improvement. If an independent local correction is warranted, read back the edit and compare the same bars using compare_to and a neutral aspect, omitting focus. Human listening decides musical quality. IDs retain only the latest four excerpts."}).to_string();
        Ok(ListeningResult {
            owner: self.owner,
            excerpt,
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

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

    fn critic(
        replies: &[&str],
    ) -> (
        AudioReviewOptions,
        std::thread::JoinHandle<Vec<(String, serde_json::Value)>>,
    ) {
        let (url, worker) = crate::audio_review::tests::server(
            replies
                .iter()
                .map(|reply| (200, crate::audio_review::tests::response(reply)))
                .collect(),
        );
        (
            AudioReviewOptions {
                configured: true,
                auto_select: false,
                url: format!("{url}/v1"),
                model: "scripted-critic".into(),
                api_key: None,
                ollama_url: None,
            },
            worker,
        )
    }

    fn listen(
        session: &mut Session,
        options: &AudioReviewOptions,
        previous: Option<&str>,
    ) -> serde_json::Value {
        let job = session
            .begin_listening_with_options(
                1,
                1,
                None,
                previous,
                previous.map(|_| ComparisonAspect::Balance),
                options.clone(),
            )
            .unwrap();
        let result = job.run_with_order(&AtomicBool::new(false), true).unwrap();
        serde_json::from_str(&session.continue_listening(result).unwrap()).unwrap()
    }

    fn submitted(request: &serde_json::Value) -> Vec<Vec<u8>> {
        request["messages"][0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|part| part["type"] == "input_audio")
            .map(|part| {
                base64::engine::general_purpose::STANDARD
                    .decode(part["input_audio"]["data"].as_str().unwrap())
                    .unwrap()
            })
            .collect()
    }

    fn rms(bytes: &[u8]) -> f64 {
        let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, 48000);
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
    fn unsaved_listen_edit_compare_submits_real_audio_and_keeps_the_baseline() {
        let (mut session, track) = fixture();
        let original = session.project().clone();
        let original_revision = session.revision();
        let (options, worker) = critic(&[
            r#"{"kind":"diagnostic","status":"cannot_judge","observations":[],"uncertainty":"Scripted transport only","suggestion":null}"#,
            r#"{"kind":"comparison","verdict":"prefer_a","confidence":"low","observations":[{"excerpt":"both","start_seconds":0,"end_seconds":1,"detail":"A is quieter."}],"reason":"Scripted preference","uncertainty":"Not a listening test"}"#,
        ]);
        let first = listen(&mut session, &options, None);
        assert_eq!(session.project(), &original);
        assert_eq!(session.revision(), original_revision);
        assert!(session.path().is_none());
        assert_eq!(first["audio_sent"], true);
        assert_eq!(first["assessment_status"], "unvalidated_response");
        let baseline = session.listening.excerpts[0].clone();
        let baseline_audio = baseline.audio.clone();
        session.set_param(crate::ParamTarget::TrackGain(track), -12.0);
        let second = listen(&mut session, &options, first["audition_id"].as_str());
        assert_eq!(second["compare_to"], first["audition_id"]);
        assert_eq!(second["before"]["revision"], original_revision);
        assert!(second["revision"].as_u64().unwrap() > original_revision);
        assert_eq!(baseline.audio.channels(), baseline_audio.channels());
        assert_eq!(
            second["audio"]["attenuation_db"],
            second["before"]["audio"]["attenuation_db"]
        );
        let requests = worker.join().unwrap();
        let original_upload = submitted(&requests[0].1);
        let comparison = submitted(&requests[1].1);
        assert_eq!(original_upload[0], comparison[1]);
        assert!((20.0 * (rms(&comparison[0]) / rms(&comparison[1])).log10() + 12.0).abs() < 0.02);
        assert!(second["comparison"]["reported_preference"].is_null());
        assert_eq!(second["comparison"]["calibrated"], false);
        assert!(second["assessment_error"].is_null());
        assert_eq!(second["assessment_status"], "unvalidated_response");
        let prompt = requests[1].1["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(prompt.starts_with("Audio excerpts in order:\n1. A\n2. B\n"));
        assert!(
            !prompt.contains("Current") && !prompt.contains("Before") && !prompt.contains("-12")
        );
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
        let (options, worker) = critic(&["Initial", "Comparison"]);
        let first = listen(&mut session, &options, None);
        let original = session.listening.excerpts[0].audio.clone();
        session.set_param(crate::ParamTarget::TrackGain(track), 30.0);
        let second = listen(&mut session, &options, first["audition_id"].as_str());
        assert!(
            second["audio"]["source_full_scale_samples"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(second["audio"]["attenuation_db"].as_f64().unwrap() < 0.0);
        assert_eq!(
            second["audio"]["attenuation_db"],
            second["before"]["audio"]["attenuation_db"]
        );
        assert!(second["audio"]["true_peak_estimate_dbfs"].as_f64().unwrap() < -0.99);
        assert_eq!(
            session.listening.excerpts[0].audio.channels(),
            original.channels()
        );
        assert_eq!(session.project().track(track).unwrap().mixer.gain_db, 30.0);
        let requests = worker.join().unwrap();
        let pair = submitted(&requests[1].1);
        assert!((20.0 * (rms(&pair[0]) / rms(&pair[1])).log10() - 30.0).abs() < 0.1);
        assert!(second["assessment"].is_null());
        assert_eq!(second["review_format"], "plain_text");
        assert_eq!(second["review"], "Comparison");
        assert!(second["comparison"]["reported_preference"].is_null());
    }

    #[test]
    fn stale_results_invalid_comparisons_and_cancellation_do_not_publish() {
        let (mut session, track) = fixture();
        let (options, worker) = critic(&["Initial", "Stale observation"]);
        let first = listen(&mut session, &options, None);
        let baseline = first["audition_id"].as_str().unwrap();
        assert!(
            session
                .begin_listening_with_options(
                    2,
                    1,
                    None,
                    Some(baseline),
                    Some(ComparisonAspect::Balance),
                    options.clone()
                )
                .is_err()
        );
        assert!(
            session
                .begin_listening_with_options(
                    1,
                    1,
                    None,
                    Some("C:/external.wav"),
                    Some(ComparisonAspect::Balance),
                    options.clone()
                )
                .is_err()
        );
        assert!(
            session
                .begin_listening_with_options(1, 9, None, None, None, options.clone())
                .is_err()
        );
        session
            .agent_command(crate::live_agent::Command::EditTimeline {
                operation: crate::live_agent::document_editing::TimelineOperation::SetTempo {
                    at_tick: 0,
                    bpm: 90.0,
                },
            })
            .unwrap();
        assert!(
            session
                .begin_listening_with_options(
                    1,
                    1,
                    None,
                    Some(baseline),
                    Some(ComparisonAspect::Balance),
                    options.clone()
                )
                .is_err()
        );
        session.undo();
        let cancelled = session
            .begin_listening_with_options(1, 1, None, None, None, options.clone())
            .unwrap();
        assert!(cancelled.run(&AtomicBool::new(true)).is_err());
        let job = session
            .begin_listening_with_options(
                1,
                1,
                None,
                Some(baseline),
                Some(ComparisonAspect::Balance),
                options.clone(),
            )
            .unwrap();
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
        assert!(
            session
                .begin_listening_with_options(
                    1,
                    1,
                    None,
                    Some(baseline),
                    Some(ComparisonAspect::Balance),
                    options
                )
                .is_err()
        );
        assert_eq!(
            worker.join().unwrap().len(),
            2,
            "cancelled work must not upload audio"
        );
    }

    #[test]
    fn old_ids_are_evicted_and_other_sessions_cannot_accept_the_result() {
        let (mut session, _) = fixture();
        let (options, worker) = critic(&["1", "2", "3", "4", "5", "6"]);
        let first = listen(&mut session, &options, None);
        for _ in 0..RETAINED_EXCERPTS {
            listen(&mut session, &options, None);
        }
        assert_eq!(session.listening.excerpts.len(), RETAINED_EXCERPTS);
        assert!(
            session
                .begin_listening_with_options(
                    1,
                    1,
                    None,
                    first["audition_id"].as_str(),
                    Some(ComparisonAspect::Balance),
                    options.clone()
                )
                .is_err()
        );
        let result = session
            .begin_listening_with_options(1, 1, None, None, None, options)
            .unwrap()
            .run(&AtomicBool::new(false))
            .unwrap();
        let (mut other, _) = fixture();
        assert!(other.continue_listening(result).is_err());
        assert_eq!(worker.join().unwrap().len(), 6);
    }

    #[test]
    fn nonfinite_audio_is_rejected_before_wav_encoding() {
        let audio = AudioBuffer::from_planar(vec![vec![f32::NAN]], 48000.0).unwrap();
        let target = tempfile::NamedTempFile::new().unwrap();
        assert!(write_audition_wav(target.path(), &audio, &AtomicBool::new(false)).is_err());
        assert_eq!(target.as_file().metadata().unwrap().len(), 0);
    }

    #[test]
    fn comparison_refuses_editing_narrative_or_missing_aspect_before_rendering() {
        let (mut session, _) = fixture();
        let before = session.project().clone();
        for (focus, baseline, aspect) in [
            (
                Some("I improved the balance; confirm it"),
                Some("id"),
                Some(ComparisonAspect::Balance),
            ),
            (None, Some("id"), None),
            (None, None, Some(ComparisonAspect::Balance)),
        ] {
            let error = session
                .begin_listening_with_options(
                    1,
                    1,
                    focus,
                    baseline,
                    aspect,
                    AudioReviewOptions::default(),
                )
                .err()
                .unwrap();
            assert!(error.contains("Comparison requires"));
            assert_eq!(session.project(), &before);
        }
    }

    #[test]
    fn anonymous_order_preserves_audio_without_extracting_a_json_looking_verdict() {
        let (mut session, track) = fixture();
        let comparison = r#"{"kind":"comparison","verdict":"prefer_b","confidence":"low","observations":[{"excerpt":"both","start_seconds":0,"end_seconds":1,"detail":"Different levels"}],"reason":"Scripted","uncertainty":"Transport test only"}"#;
        let (options, worker) = critic(&["Unstructured baseline", comparison, comparison]);
        let first = listen(&mut session, &options, None);
        session.set_param(crate::ParamTarget::TrackGain(track), -12.0);
        for current_first in [false, true] {
            let job = session
                .begin_listening_with_options(
                    1,
                    1,
                    None,
                    first["audition_id"].as_str(),
                    Some(ComparisonAspect::Balance),
                    options.clone(),
                )
                .unwrap();
            let result = job
                .run_with_order(&AtomicBool::new(false), current_first)
                .unwrap();
            let reply: serde_json::Value =
                serde_json::from_str(&session.continue_listening(result).unwrap()).unwrap();
            assert!(reply["comparison"]["reported_preference"].is_null());
            assert_eq!(reply["review"], comparison);
            assert_eq!(
                reply["comparison"]["b"],
                if current_first { "baseline" } else { "current" }
            );
        }
        let requests = worker.join().unwrap();
        let first_pair = submitted(&requests[1].1);
        let second_pair = submitted(&requests[2].1);
        assert_eq!(first_pair[0], second_pair[1]);
        assert_eq!(first_pair[1], second_pair[0]);
    }
}
