//! Preparing source analysis without moving the session onto a worker.

use std::sync::Arc;

use auris_core::{AudioBuffer, SourceId};
use auris_dsp::Spectrogram;

use super::Session;

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
}
