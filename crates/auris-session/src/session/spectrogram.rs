//! Preparing source analysis without moving the session onto a worker.

use std::sync::Arc;

use auris_core::{AudioBuffer, SourceId, TrackId};
use auris_dsp::Spectrogram;

use super::Session;

/// A render and analysis of a document snapshot, ready to move to a worker.
///
/// A track is heard through its solo routing, including buses, sends and master effects.
/// With no track selected the result is the current project mix, including mute and solo.
pub struct RenderedSpectrogramJob {
    render: crate::RenderJob,
    track: Option<TrackId>,
    revision: u64,
}

impl RenderedSpectrogramJob {
    /// Document revision captured by this job; discard results after the session changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Renders and analyses the whole arrangement, including effect tails, on a worker.
    pub fn run(
        mut self,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Spectrogram, crate::SessionError> {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(auris_engine::EngineError::RenderCancelled.into());
        }
        let audio = self.render.render_target(
            self.track,
            &auris_engine::OfflineOptions::whole_project(),
            &mut auris_engine::RenderProgress::default().cancelled_by(cancel),
        )?;
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(auris_engine::EngineError::RenderCancelled.into());
        }
        Ok(Spectrogram::analyse(&audio))
    }
}

/// Immutable source audio prepared for spectrogram analysis on a worker thread.
///
/// Cloning a job shares the source samples, so a frontend can retain one copy for cache
/// validation and send another to its worker. Use [`Session::spectrogram_job_is_current`]
/// before publishing a result: the source may have been replaced or its project closed.
#[derive(Clone, Debug)]
pub struct SpectrogramJob {
    source: SourceId,
    audio: Arc<AudioBuffer>,
}

impl SpectrogramJob {
    /// The source this job analyses.
    pub fn source(&self) -> SourceId {
        self.source
    }

    /// Computes the source's spectrogram; call this on a worker, never the UI or audio thread.
    pub fn run(&self) -> Spectrogram {
        Spectrogram::analyse(&self.audio)
    }
}

impl Session {
    /// Prepares a spectrogram of a track's rendered sound, or the whole mix for `None`.
    ///
    /// This snapshots the document and creates independent hosted plugin instances on their
    /// owning thread. Rendering and FFTs happen in [`RenderedSpectrogramJob::run`]. The session,
    /// playback and undo history are unchanged. A removed track returns `None`.
    pub fn rendered_spectrogram_job(
        &mut self,
        track: Option<TrackId>,
    ) -> Option<RenderedSpectrogramJob> {
        if track.is_some_and(|id| self.project.track(id).is_none()) {
            return None;
        }
        Some(RenderedSpectrogramJob {
            revision: self.revision(),
            render: self.render_job(),
            track,
        })
    }

    /// Shares a loaded source's audio for offline spectrogram analysis.
    ///
    /// This only clones an `Arc`; [`SpectrogramJob::run`] does the expensive work later.
    /// Missing sources return `None`, including sources no longer in the document but still
    /// retained in the audio bank for undo.
    pub fn spectrogram_job(&self, source: SourceId) -> Option<SpectrogramJob> {
        if !self.project.audio_sources.contains_key(&source) {
            return None;
        }
        Some(SpectrogramJob {
            source,
            audio: Arc::clone(self.bank.get(source)?),
        })
    }

    /// Whether a pending or cached spectrogram still describes the source in this document.
    ///
    /// Source IDs alone are insufficient: relinking or reloading can replace a buffer while
    /// keeping its ID. Pointer identity also distinguishes a newly opened document from an
    /// older job without comparing or copying the source samples.
    pub fn spectrogram_job_is_current(&self, job: &SpectrogramJob) -> bool {
        self.project.audio_sources.contains_key(&job.source)
            && self
                .bank
                .get(job.source)
                .is_some_and(|audio| Arc::ptr_eq(audio, &job.audio))
    }
}

#[cfg(test)]
mod tests {
    use auris_core::AssetPath;

    use super::*;
    use crate::SessionOptions;

    fn source(session: &mut Session) -> SourceId {
        let id = session.project.add_audio_source(
            "Spectrum",
            AssetPath::external("spectrum.wav"),
            4_096,
            48_000.0,
            1,
        );
        session
            .bank
            .insert(id, Arc::new(AudioBuffer::new(1, 4_096, 48_000.0)));
        id
    }

    #[test]
    fn worker_result_is_valid_only_for_the_same_source_buffer() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let source = source(&mut session);
        let job = session.spectrogram_job(source).unwrap();
        assert_eq!(job.source(), source);
        assert!(session.spectrogram_job_is_current(&job));
        let worker_job = job.clone();
        let result = std::thread::spawn(move || worker_job.run()).join().unwrap();
        assert_eq!(result.columns(), 2);
        assert_eq!(result.frame_count(), 4_096);
        assert!(session.spectrogram_job_is_current(&job));

        session
            .bank
            .insert(source, Arc::new(AudioBuffer::new(1, 4_096, 48_000.0)));
        assert!(!session.spectrogram_job_is_current(&job));
        assert!(session.spectrogram_job_is_current(&session.spectrogram_job(source).unwrap()));
    }

    #[test]
    fn removed_or_missing_sources_do_not_produce_current_jobs() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let source = source(&mut session);
        let job = session.spectrogram_job(source).unwrap();
        session.project.audio_sources.remove(&source);
        assert!(session.spectrogram_job(source).is_none());
        assert!(!session.spectrogram_job_is_current(&job));

        let source = self::source(&mut session);
        session.bank.remove(source);
        assert!(session.spectrogram_job(source).is_none());
    }

    fn peak(spectrum: &Spectrogram) -> f32 {
        (0..spectrum.columns())
            .flat_map(|column| spectrum.column(column).unwrap())
            .copied()
            .fold(auris_dsp::SILENCE_DB, f32::max)
    }

    fn analyse(session: &mut Session, track: Option<TrackId>) -> Spectrogram {
        session
            .rendered_spectrogram_job(track)
            .unwrap()
            .run(&std::sync::atomic::AtomicBool::new(false))
            .unwrap()
    }

    #[test]
    fn rendered_spectrograms_perform_every_note_track_kind_without_editing() {
        use auris_core::{Note, Ticks};
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let instrument = session.add_default_instrument_track("Keys").unwrap();
        let drum = session.add_default_drum_track("Drums").unwrap();
        let singer = session.add_singer_track("Singer");
        for track in [instrument, drum, singer] {
            let clip = session
                .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER)
                .unwrap();
            session
                .add_note(
                    clip,
                    Note::new(
                        if track == drum { 36 } else { 60 },
                        Ticks::ZERO,
                        Ticks::QUARTER,
                    ),
                )
                .unwrap();
        }
        let project = session.project().clone();
        let revision = session.revision();
        for track in [instrument, drum, singer] {
            let job = session.rendered_spectrogram_job(Some(track)).unwrap();
            assert_eq!(job.revision(), revision);
            let spectrum =
                std::thread::spawn(move || job.run(&std::sync::atomic::AtomicBool::new(false)))
                    .join()
                    .unwrap()
                    .unwrap();
            assert!(peak(&spectrum) > -60.0, "track {} was silent", track.0);
        }
        assert_eq!(session.project(), &project);
        assert_eq!(session.revision(), revision);
        let old = session.rendered_spectrogram_job(None).unwrap();
        session.remove_track(instrument).unwrap();
        assert_ne!(session.revision(), old.revision());
        assert!(session.rendered_spectrogram_job(Some(instrument)).is_none());
    }

    #[test]
    fn track_bus_and_project_spectra_follow_the_rendered_mix() {
        use auris_core::{Output, Ticks};
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let rate = session.project().sample_rate;
        let samples: Vec<f32> = (0..8_192)
            .map(|frame| {
                (std::f64::consts::TAU * 1_000.0 * frame as f64 / rate).sin() as f32 * 0.25
            })
            .collect();
        let mut tracks = Vec::new();
        for name in ["one.wav", "two.wav"] {
            let clip = session
                .place_audio(
                    std::path::Path::new(name),
                    AudioBuffer::from_planar(vec![samples.clone()], rate).unwrap(),
                    Ticks::ZERO,
                )
                .unwrap();
            tracks.push(session.track_of_clip(clip).unwrap());
        }
        let bus = session.add_bus_track("Bus");
        session
            .set_track_output(tracks[0], Output::Bus(bus))
            .unwrap();
        let single = peak(&analyse(&mut session, Some(tracks[0])));
        let bussed = peak(&analyse(&mut session, Some(bus)));
        let mix = peak(&analyse(&mut session, None));
        assert!((single - bussed).abs() < 0.01);
        assert!((mix - single - 6.0206).abs() < 0.05, "{single} vs {mix}");
        session.set_track_solo(tracks[0], true).unwrap();
        assert!((peak(&analyse(&mut session, None)) - single).abs() < 0.05);
        session.set_track_solo(tracks[0], false).unwrap();
        session.set_track_mute(tracks[1], true).unwrap();
        assert!((peak(&analyse(&mut session, None)) - single).abs() < 0.05);
        session.set_track_mute(tracks[0], true).unwrap();
        assert_eq!(peak(&analyse(&mut session, None)), auris_dsp::SILENCE_DB);
    }
}
