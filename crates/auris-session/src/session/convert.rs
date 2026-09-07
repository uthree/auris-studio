//! Render a note track's source while retaining its editable mixer and routing.

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use auris_core::param::ParamTarget;
use auris_core::project::AudioTrack;
use auris_core::time::Ticks;
use auris_core::{AssetPath, AudioBuffer, ClipId, MixerStrip, Output, Project, TrackId, TrackKind};
use auris_engine::{OfflineOptions, RenderProgress};
use auris_io::{WavBitDepth, WavExportSettings, resample_buffer, write_wav};

use super::{PlaybackState, Session, SingPlan, SingerTakeState};
use crate::{RenderJob, SessionError, VoiceModel, history::Edit};

// Offline conversion bounds. A source that has not settled within this window is refused
// rather than saved with a cut-off release.
const SOURCE_TAIL_SECONDS: f64 = 10.0;
const SOURCE_QUIET_SECONDS: f64 = 0.05;
const SOURCE_SILENCE: f32 = 1e-6;

enum Source {
    Instrument(Box<RenderJob>),
    Take(Arc<AudioBuffer>),
    Singer(Box<SingPlan>, Arc<Mutex<VoiceModel>>),
}

/// A detached render prepared by [`Session::convert_track_job`].
pub struct TrackConversionJob {
    source: Source,
    block_frames: usize,
    original: Project,
    revision: u64,
    folder: PathBuf,
    track: TrackId,
}

/// Rendered audio awaiting an atomic, undoable replacement in its originating session.
pub struct TrackConversion {
    original: Project,
    revision: u64,
    folder: PathBuf,
    track: TrackId,
    buffer: AudioBuffer,
}

impl TrackConversionJob {
    /// Renders off the session thread, with progress and cancellation between blocks/chunks.
    pub fn render(
        self,
        report: &mut dyn FnMut(f32),
        cancel: &AtomicBool,
    ) -> Result<TrackConversion, SessionError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(auris_engine::EngineError::RenderCancelled.into());
        }
        let buffer = match self.source {
            Source::Instrument(mut job) => {
                let end = self
                    .original
                    .tempo_map
                    .ticks_to_samples(self.original.end_tick(), self.original.sample_rate)
                    .raw();
                let tail = (self.original.sample_rate * SOURCE_TAIL_SECONDS).round() as u64;
                let padded_end = end.checked_add(tail).ok_or_else(|| {
                    SessionError::TrackConversion("the source render is too long".into())
                })?;
                let buffer = job.render(
                    &OfflineOptions {
                        // Hosted instances were prepared by job_for with this maximum block size.
                        block_frames: self.block_frames,
                        ..OfflineOptions::default()
                    }
                    .with_range(0, padded_end),
                    &mut RenderProgress::reporting(report).cancelled_by(cancel),
                )?;
                trim_source_tail(buffer, end as usize)?
            }
            Source::Take(buffer) => (*buffer).clone(),
            Source::Singer(plan, model) => {
                let samples = model
                    .lock()
                    .expect("no thread panics holding a voice")
                    .sing_score_with(
                        &plan.frames,
                        &plan.score,
                        plan.speaker,
                        plan.seed,
                        |done, total| {
                            report(done as f32 / total.max(1) as f32);
                            !cancel.load(Ordering::Relaxed)
                        },
                    )?;
                AudioBuffer::from_planar(vec![samples], f64::from(plan.sample_rate))?
            }
        };
        let buffer = if buffer.sample_rate() == self.original.sample_rate {
            buffer
        } else {
            resample_buffer(&buffer, self.original.sample_rate)?
        };
        validate_conversion_audio(&buffer)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(auris_engine::EngineError::RenderCancelled.into());
        }
        report(1.0);
        Ok(TrackConversion {
            original: self.original,
            revision: self.revision,
            folder: self.folder,
            track: self.track,
            buffer,
        })
    }
}

fn validate_conversion_audio(buffer: &AudioBuffer) -> Result<(), SessionError> {
    if buffer
        .channels()
        .iter()
        .flatten()
        .any(|sample| !sample.is_finite())
    {
        return Err(SessionError::TrackConversion(
            "the source produced non-finite audio".into(),
        ));
    }
    Ok(())
}

fn trim_source_tail(mut buffer: AudioBuffer, end: usize) -> Result<AudioBuffer, SessionError> {
    validate_conversion_audio(&buffer)?;
    let quiet_frames = (buffer.sample_rate() * SOURCE_QUIET_SECONDS)
        .round()
        .max(1.0) as usize;
    let quiet_start = buffer.frame_count().saturating_sub(quiet_frames);
    if buffer.channels().iter().any(|channel| {
        channel[quiet_start..]
            .iter()
            .any(|sample| sample.abs() > SOURCE_SILENCE)
    }) {
        return Err(SessionError::TrackConversion(
            "the source is still sounding after the conversion tail limit".into(),
        ));
    }
    let frames = buffer
        .channels()
        .iter()
        .filter_map(|channel| {
            channel
                .iter()
                .rposition(|sample| sample.abs() > SOURCE_SILENCE)
        })
        .max()
        .map_or(end, |last| end.max(last + 1));
    buffer.set_frame_count(frames);
    Ok(buffer)
}

impl Session {
    /// Prepares conversion of an instrument, drum or singer track to audio.
    ///
    /// Only the source is baked: instrument automation is rendered, while the mixer, effects,
    /// sends and their automation remain editable. Muting/soloing does not silence the render.
    /// A current singer take is reused; an absent or stale take is sung through the chosen voice.
    /// Native plugin state is captured for undo. The job retains the session revision so native
    /// edits made while it renders invalidate its result too. Instrument releases are retained;
    /// a source that has not settled within the bounded tail window is refused.
    /// Audio tracks, buses, empty scores and unavailable instruments are refused before editing.
    pub fn convert_track_job(
        &mut self,
        track: TrackId,
    ) -> Result<TrackConversionJob, SessionError> {
        self.require_track(track)?;
        let entry = self.project.track(track).expect("checked track");
        if !entry.kind.holds_notes()
            || entry.end_tick(&self.project.tempo_map, self.project.sample_rate) <= Ticks::ZERO
        {
            return Err(SessionError::TrackConversion(
                "choose a non-empty instrument, drum or singer track".into(),
            ));
        }
        self.poll();
        self.collect_hosted_state();
        let original = self.project.clone();
        let revision = self.revision();
        let entry = original.track(track).expect("checked track");
        let source = if entry.kind.is_singer() {
            let take = entry
                .kind
                .as_singer()
                .and_then(|singer| singer.take.as_ref());
            let buffer = take.and_then(|take| self.bank.get(take.source)).cloned();
            if self.singer_take_state(track)? == SingerTakeState::Current
                && let Some(buffer) = buffer
            {
                Source::Take(buffer)
            } else {
                let plan = self.sing_plan(track, None)?;
                let model = self.voice_model_at(&plan.voice)?;
                Source::Singer(Box::new(plan), model)
            }
        } else {
            if self.playback_readiness().iter().any(|readiness| {
                readiness.track == track
                    && matches!(
                        readiness.state,
                        PlaybackState::MissingInstrument | PlaybackState::MissingSoundfont
                    )
            }) {
                return Err(SessionError::TrackConversion(
                    "the track's instrument or SoundFont is unavailable".into(),
                ));
            }
            let mut project = original.clone();
            project.tracks.retain(|entry| entry.id == track);
            let entry = &mut project.tracks[0];
            entry.mixer = MixerStrip::default();
            entry.output = Output::Master;
            entry.sends.clear();
            project.master = MixerStrip::default();
            project.automation.remove_lanes_where(|target| !matches!(target, ParamTarget::Instrument { track: id, .. } if id == track));
            // Keep the complete document in the hosted-plugin manager: a subset would retire
            // other tracks' live plugin handles as though those tracks had been deleted.
            let mut job = self.job_for(original.clone());
            job.restrict_to_source(project, track);
            if original
                .track(track)
                .and_then(|entry| entry.kind.as_instrument())
                .is_some_and(|instrument| instrument.is_hosted())
                && !job.has_placed_instrument(track)
            {
                return Err(SessionError::TrackConversion(
                    "the hosted instrument could not be prepared for rendering".into(),
                ));
            }
            Source::Instrument(Box::new(job))
        };
        Ok(TrackConversionJob {
            source,
            block_frames: self.engine.max_block(),
            original,
            revision,
            folder: self.project_folder().expect("working folder").to_path_buf(),
            track,
        })
    }

    /// Converts a track synchronously; a GUI uses [`Self::convert_track_job`] on a worker.
    pub fn convert_track_to_audio(&mut self, track: TrackId) -> Result<ClipId, SessionError> {
        let result = self
            .convert_track_job(track)?
            .render(&mut |_| {}, &AtomicBool::new(false))?;
        self.land_track_conversion(result)
    }

    /// Writes float WAV audio and replaces the source track in one undo step.
    ///
    /// A changed document, native plugin state or project folder invalidates the result. Failure
    /// before the file is complete leaves the document and history untouched. The stable track id
    /// keeps routing, sidechains, ordering, colour and mixer settings intact. Undo restores the score.
    pub fn land_track_conversion(
        &mut self,
        result: TrackConversion,
    ) -> Result<ClipId, SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        self.poll();
        if self.revision() != result.revision
            || self.project != result.original
            || self.project_folder() != Some(result.folder.as_path())
        {
            return Err(SessionError::TrackConversion(
                "the project changed during rendering; convert the track again".into(),
            ));
        }
        let name = self
            .project
            .track(result.track)
            .expect("unchanged document")
            .name
            .clone();
        let audio_dir = result.folder.join(auris_io::AUDIO_DIR);
        std::fs::create_dir_all(&audio_dir)
            .map_err(|error| auris_io::IoError::from_fs(&audio_dir, error))?;
        let inside = PathBuf::from(auris_io::AUDIO_DIR)
            .join(super::record::take_file_name(&result.folder, &name));
        let path = result.folder.join(&inside);
        let settings = WavExportSettings {
            bit_depth: WavBitDepth::Float32,
            sample_rate: result.buffer.sample_rate().round() as u32,
            dither: false,
        };
        if let Err(error) = write_wav(&path, &result.buffer, &settings) {
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
        self.record(Edit::ConvertTrackToAudio);
        let source = self.project.add_audio_source(
            name,
            AssetPath::inside(inside),
            result.buffer.frame_count() as u64,
            result.buffer.sample_rate(),
            result.buffer.channel_count(),
        );
        self.record_source_size(source, &path);
        self.project
            .track_mut(result.track)
            .expect("unchanged track")
            .kind = TrackKind::Audio(AudioTrack::default());
        self.project.remove_instrument_automation(result.track);
        let clip = self
            .project
            .add_audio_clip(result.track, source, Ticks::ZERO)
            .expect("audio track");
        self.install_source(source, Arc::new(result.buffer));
        self.invalidate_graph();
        Ok(clip)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{Scratch, session};
    use super::*;
    use auris_core::automation::AutomationCurve;
    use auris_core::{Note, SingerTake, SingerVoice};

    fn instrument() -> (Session, TrackId) {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::QUARTER, Ticks::from_beats(2.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        (session, track)
    }

    fn render(session: &mut Session) -> AudioBuffer {
        session
            .render_job()
            .render(&OfflineOptions::default(), &mut RenderProgress::default())
            .unwrap()
    }

    #[test]
    fn conversion_preserves_the_mix_timing_routing_and_undo() {
        let (mut session, track) = instrument();
        let bus = session.project.add_bus_track("Bus");
        let entry = session.project.track_mut(track).unwrap();
        entry.output = Output::Bus(bus);
        entry.mixer.gain_db = -9.0;
        entry.mixer.pan = 0.4;
        session.project.master.gain_db = -3.0;
        session.project.automation.set_point(
            ParamTarget::TrackGain(track),
            None,
            AutomationCurve::Linear,
            Ticks::ZERO,
            -6.0,
        );
        session.project.automation.set_point(
            ParamTarget::TrackGain(track),
            None,
            AutomationCurve::Linear,
            Ticks::from_beats(3.0),
            -12.0,
        );
        // A later track extends the arrangement, and must never leak into the baked source.
        let other = session.add_default_instrument_track("Later").unwrap();
        let later = session
            .add_midi_clip(
                other,
                "Later phrase",
                Ticks::from_beats(4.0),
                Ticks::from_beats(2.0),
            )
            .unwrap();
        session
            .add_note(later, Note::new(72, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.add_effect(Some(track), "auris.fx.gain").unwrap();
        session.project.track_mut(track).unwrap().mixer.effects[0]
            .state
            .params
            .insert("gain_db".into(), -6.0);
        let before = session.project.clone();
        let expected = render(&mut session);
        assert!(expected.peak() > 0.001);
        let clip = session.convert_track_to_audio(track).unwrap();
        let converted = session.project.clone();
        let entry = converted.track(track).unwrap();
        assert_eq!(entry.mixer, before.track(track).unwrap().mixer);
        assert_eq!(entry.output, Output::Bus(bus));
        assert_eq!(converted.tracks.len(), before.tracks.len());
        assert_eq!(entry.kind.as_audio().unwrap().clips[0].id, clip);
        assert_eq!(entry.kind.as_audio().unwrap().clips[0].start, Ticks::ZERO);
        let actual = render(&mut session);
        assert_eq!(actual.frame_count(), expected.frame_count());
        for channel in 0..expected.channel_count() {
            let error = expected
                .channel(channel)
                .iter()
                .zip(actual.channel(channel))
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(error < 1e-5, "conversion changed the mix by {error}");
        }
        assert_eq!(session.undo(), Some(Edit::ConvertTrackToAudio));
        assert_eq!(session.project, before);
        assert_eq!(session.redo(), Some(Edit::ConvertTrackToAudio));
        assert_eq!(session.project, converted);
    }

    #[test]
    fn conversion_preserves_a_final_notes_release_through_retained_effects() {
        let mut session = session();
        let track = session.add_default_instrument_track("Last note").unwrap();
        let clip = session
            .add_midi_clip(track, "Held to the end", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let instrument = session
            .project
            .track_mut(track)
            .unwrap()
            .kind
            .as_instrument_mut()
            .unwrap();
        for (key, value) in [
            ("attack", 0.001),
            ("decay", 0.001),
            ("sustain", 1.0),
            ("release", 0.25),
        ] {
            instrument.instrument_state.params.insert(key.into(), value);
        }
        session.add_effect(Some(track), "auris.fx.delay").unwrap();
        let effect = &mut session.project.track_mut(track).unwrap().mixer.effects[0];
        for (key, value) in [("time_ms", 500.0), ("feedback", 0.0), ("mix", 0.5)] {
            effect.state.params.insert(key.into(), value);
        }
        let mixer = session.project.track(track).unwrap().mixer.clone();
        let end = session
            .project
            .tempo_map
            .ticks_to_samples(session.project.end_tick(), session.project.sample_rate)
            .raw() as usize;
        let expected = render(&mut session);
        assert_eq!(end, 24_000);
        assert_eq!(expected.frame_count(), 48_000);
        let release_end = end + 6_000;
        let release_peak = expected.channel(0)[end..release_end]
            .iter()
            .copied()
            .map(f32::abs)
            .fold(0.0f32, f32::max);
        assert!(
            release_peak > 0.01,
            "the original must contain an audible release"
        );

        session.convert_track_to_audio(track).unwrap();
        assert_eq!(session.project.track(track).unwrap().mixer, mixer);
        let converted = &session
            .project
            .track(track)
            .unwrap()
            .kind
            .as_audio()
            .unwrap()
            .clips[0];
        assert!(
            converted.length_frames > end as u64,
            "the audio clip must retain the release"
        );
        let actual = render(&mut session);
        assert!(actual.frame_count() >= expected.frame_count());
        let error = expected.channel(0)[..release_end]
            .iter()
            .zip(&actual.channel(0)[..release_end])
            .map(|(before, after)| (before - after).abs())
            .fold(0.0f32, f32::max);
        assert!(
            error < 1e-5,
            "conversion cut the final release: error {error}, before {} frames, after {} frames",
            expected.frame_count(),
            actual.frame_count(),
        );
    }

    #[test]
    fn source_tail_validation_refuses_invalid_or_unfinished_audio() {
        let end = 400;
        for (frame, value) in [
            (0, f32::NAN),
            (200, f32::INFINITY),
            (600, f32::NEG_INFINITY),
        ] {
            let mut buffer = AudioBuffer::stereo(1_000, 1_000.0);
            buffer.channel_mut(1)[frame] = value;
            assert!(trim_source_tail(buffer, end).is_err());
        }
        let mut unfinished = AudioBuffer::stereo(1_000, 1_000.0);
        unfinished.channel_mut(0)[999] = 0.01;
        assert!(trim_source_tail(unfinished, end).is_err());

        let mut released = AudioBuffer::stereo(1_000, 1_000.0);
        released.channel_mut(1)[450] = 0.25;
        let released = trim_source_tail(released, end).unwrap();
        assert_eq!(released.frame_count(), 451);
        assert_eq!(released.channel(1)[450], 0.25);
        assert_eq!(
            trim_source_tail(AudioBuffer::stereo(1_000, 1_000.0), end)
                .unwrap()
                .frame_count(),
            end,
            "silence must not shorten the original arrangement"
        );
    }

    #[test]
    fn muted_drums_render_their_source_and_bake_only_instrument_automation() {
        let (mut session, track) = instrument();
        let instrument = session
            .project
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clone();
        session.project.track_mut(track).unwrap().kind = TrackKind::Drum(instrument);
        session.project.track_mut(track).unwrap().mixer.mute = true;
        let other = session.project.add_bus_track("Soloed elsewhere");
        session.project.track_mut(other).unwrap().mixer.solo = true;
        let param = ParamTarget::Instrument {
            track,
            param: auris_core::param::ParamId(0),
        };
        session
            .project
            .automation
            .set_point(param, None, AutomationCurve::Hold, Ticks::ZERO, 0.5);
        let job = session.convert_track_job(track).unwrap();
        let Source::Instrument(ref render) = job.source else {
            panic!("instrument job")
        };
        assert!(render.project().automation.lane(param).is_some());
        let result = job.render(&mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert!(result.buffer.peak() > 0.001);
        session.land_track_conversion(result).unwrap();
        assert!(session.project.track(track).unwrap().mixer.mute);
        assert!(session.project.automation.lane(param).is_none());
    }

    #[test]
    fn converted_audio_survives_save_as_and_reopening() {
        let scratch = Scratch::new("converted-audio");
        let (mut session, track) = instrument();
        session.convert_track_to_audio(track).unwrap();
        let expected = render(&mut session);
        let document = session
            .save_as(&scratch.join("Song.auris"))
            .unwrap()
            .document;
        drop(session);
        let mut reopened = self::session();
        reopened.open(&document).unwrap();
        let actual = render(&mut reopened);
        assert_eq!(actual.channel(0), expected.channel(0));
        let source = reopened.project.audio_sources.values().next().unwrap();
        assert!(matches!(source.path, AssetPath::Inside(_)));
        assert!(
            source
                .path
                .resolve(reopened.project_folder())
                .unwrap()
                .is_file()
        );
    }

    #[test]
    fn soundfont_audio_is_available_after_its_instrument_has_been_replaced() {
        let scratch = Scratch::new("converted-soundfont");
        let (mut session, track) = instrument();
        let path = scratch.soundfont("Tone.sf2");
        // The shared font fixture contains silence. Give its sample a tone so this checks
        // actual sampler playback rather than two equally silent renders.
        let mut bytes = std::fs::read(&path).unwrap();
        let samples = bytes.windows(4).position(|tag| tag == b"smpl").unwrap() + 8;
        for frame in 0..64 {
            let sample = ((frame as f32 * std::f32::consts::TAU / 16.0).sin() * 8192.0) as i16;
            bytes[samples + frame * 2..samples + frame * 2 + 2]
                .copy_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        let font = session.import_soundfont(&path).unwrap();
        session
            .set_track_preset(
                track,
                auris_core::PresetRef {
                    font,
                    bank: 0,
                    patch: 0,
                },
            )
            .unwrap();
        let expected = render(&mut session);
        assert!(expected.peak() > 0.0);
        session.convert_track_to_audio(track).unwrap();
        let actual = render(&mut session);
        assert_eq!(actual.frame_count(), expected.frame_count());
        // Allow a few f32 rounding ulps when comparing the exporter's default blocks with
        // the live device's prepared block size.
        let error = actual
            .channel(0)
            .iter()
            .zip(expected.channel(0))
            .map(|(actual, expected)| (actual - expected).abs())
            .fold(0.0f32, f32::max);
        assert!(
            error < 1e-7,
            "SoundFont conversion changed the samples by {error}"
        );
    }

    #[test]
    fn current_singer_take_keeps_its_full_audio_without_loading_the_voice() {
        let mut session = session();
        let track = session.add_singer_track("Singer");
        let clip = session
            .add_midi_clip(track, "Words", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let source = session.project.add_audio_source(
            "Take",
            AssetPath::inside("Audio/take.wav"),
            96_000,
            48_000.0,
            1,
        );
        session.install_source(
            source,
            Arc::new(AudioBuffer::from_planar(vec![vec![0.125; 96_000]], 48_000.0).unwrap()),
        );
        session
            .project
            .track_mut(track)
            .unwrap()
            .kind
            .as_singer_mut()
            .unwrap()
            .voice = Some(SingerVoice {
            path: AssetPath::external("C:/missing-voice.onnx"),
            name: "Voice".into(),
            consonants: None,
            levels: None,
            speaker: None,
        });
        let fingerprint = session.singer_input_fingerprint(track).unwrap();
        session
            .project
            .track_mut(track)
            .unwrap()
            .kind
            .as_singer_mut()
            .unwrap()
            .take = Some(SingerTake {
            backend_pitch: None,
            source,
            fingerprint,
            seed: 0,
        });
        let before = session.project.clone();
        session.convert_track_to_audio(track).unwrap();
        let audio = session
            .project
            .track(track)
            .unwrap()
            .kind
            .as_audio()
            .unwrap();
        let buffer = session.bank.get(audio.clips[0].source).unwrap();
        assert_eq!(buffer.frame_count(), 96_000);
        assert_eq!(buffer.peak(), 0.125);
        assert_eq!(session.undo(), Some(Edit::ConvertTrackToAudio));
        assert_eq!(session.project, before);
        // Once the notes change, a missing voice must fail rather than baking the stale take.
        session.transpose_notes(clip, &[0], 4).unwrap();
        let changed = session.project.clone();
        assert!(session.convert_track_job(track).is_err());
        assert_eq!(session.project, changed);
    }

    #[test]
    fn cancellation_edits_and_invalid_tracks_never_replace_the_score() {
        let (mut session, track) = instrument();
        session.forget_history();
        let before = session.project.clone();
        let job = session.convert_track_job(track).unwrap();
        assert!(
            job.render(&mut |_| {}, &AtomicBool::new(true))
                .err()
                .unwrap()
                .is_cancellation()
        );
        assert_eq!(session.project, before);
        assert!(!session.can_undo());
        let cancel = AtomicBool::new(false);
        let job = session.convert_track_job(track).unwrap();
        let cancelled = job.render(
            &mut |fraction| {
                if fraction > 0.1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &cancel,
        );
        assert!(cancelled.err().unwrap().is_cancellation());
        assert!(!session.can_undo());
        let result = session
            .convert_track_job(track)
            .unwrap()
            .render(&mut |_| {}, &AtomicBool::new(false))
            .unwrap();
        session.rename_track(track, "Edited").unwrap();
        let changed = session.project.clone();
        assert!(session.land_track_conversion(result).is_err());
        assert_eq!(session.project, changed);
        let audio = session.add_audio_track("Audio");
        let bus = session.add_bus_track("Bus");
        let empty = session.add_default_instrument_track("Empty").unwrap();
        session.forget_history();
        for id in [audio, bus, empty, TrackId(u64::MAX)] {
            assert!(session.convert_track_job(id).is_err());
        }
        assert!(!session.can_undo());
    }

    #[test]
    fn failed_file_creation_does_not_spend_an_undo_step() {
        let (mut session, track) = instrument();
        session.forget_history();
        let before = session.project.clone();
        let result = session
            .convert_track_job(track)
            .unwrap()
            .render(&mut |_| {}, &AtomicBool::new(false))
            .unwrap();
        let audio = session.project_folder().unwrap().join(auris_io::AUDIO_DIR);
        std::fs::write(&audio, "a file prevents directory creation").unwrap();
        assert!(session.land_track_conversion(result).is_err());
        assert_eq!(session.project, before);
        assert!(!session.can_undo());
    }
}
