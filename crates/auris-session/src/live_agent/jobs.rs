//! One worker/owner protocol for agent operations that must leave the UI thread.

use super::{
    Command, VocalJob, VocalResult, VocalStep,
    file_jobs::{FileJob, FileResult},
};
use crate::{Session, Settings};
use std::sync::atomic::AtomicBool;

/// Owned work that can run without accessing the live document.
pub enum AgentJob {
    /// Bounded audio discovery, import or source processing.
    AudioAssets(Box<crate::session::audio_assets::Job>),
    /// Private candidate construction or comparison rendering.
    Candidates(crate::session::candidates::Job),
    /// Bounded backing generation against an owned document snapshot.
    Accompaniment(Box<crate::session::accompaniment::Job>),
    /// Capability-aware preparation of an immutable arrangement candidate.
    Arrangement(Box<crate::arrangement::ArrangementJob>),
    /// Voice discovery, model loading or synthesis.
    Vocal(Box<VocalJob>),
    /// File encoding, audio export or encoded-file measurement.
    File(Box<FileJob>),
    /// Snapshot serialization, history inspection or prepared restoration.
    History(Box<crate::CheckpointJob>),
    /// A read-only render measured as a modulation spectrogram.
    Modulation(Box<crate::modulation::ModulationJob>),
}

/// A worker result returned to its originating owner for validation and publication.
pub enum AgentResult {
    /// Prepared audio material awaiting its originating document.
    AudioAssets(Box<crate::session::audio_assets::Prepared>),
    /// Candidate results awaiting owner validation.
    Candidates(crate::session::candidates::Prepared),
    /// Generated backing and optional harmony awaiting their original owner.
    Accompaniment(Box<crate::session::accompaniment::Prepared>),
    /// A complete arrangement candidate awaiting its original owner.
    Arrangement(Box<crate::arrangement::ArrangementResult>),
    /// A voice worker's result.
    Vocal(VocalResult),
    /// A staged file operation's result.
    File(FileResult),
    /// Prepared history operation awaiting its owner.
    History(Box<crate::CheckpointResult>),
    /// A modulation report of the document revision it was started from.
    Modulation(Box<crate::modulation::ModulationReport>),
}

/// Another worker stage or the completed tool response.
pub enum AgentStep {
    /// Run the next detached stage.
    Work(AgentJob),
    /// The operation completed with verified readback.
    Done(String),
}

impl AgentJob {
    /// Perform detached work with cooperative cancellation.
    pub fn run(self, cancelled: &AtomicBool) -> Result<AgentResult, String> {
        match self {
            Self::AudioAssets(job) => job
                .run(cancelled)
                .map(|result| AgentResult::AudioAssets(Box::new(result))),
            Self::Candidates(job) => job.run(cancelled).map(AgentResult::Candidates),
            Self::Accompaniment(job) => job
                .run(cancelled)
                .map(|result| AgentResult::Accompaniment(Box::new(result))),
            Self::Arrangement(job) => job
                .run(cancelled)
                .map(|result| AgentResult::Arrangement(Box::new(result))),
            Self::History(job) => job
                .run(cancelled)
                .map(|result| AgentResult::History(Box::new(result))),
            Self::Vocal(job) => job.run(cancelled).map(AgentResult::Vocal),
            Self::File(job) => job.run(cancelled).map(AgentResult::File),
            Self::Modulation(job) => job
                .run(cancelled)
                .map(|report| AgentResult::Modulation(Box::new(report))),
        }
    }
}

impl Session {
    /// Begin an expensive agent operation without doing its work on the caller's UI thread.
    pub fn begin_agent_job(
        &mut self,
        command: Command,
        settings: Settings,
    ) -> Result<AgentJob, String> {
        match command {
            Command::AudioAssets { operation } => self
                .begin_audio_assets(operation)
                .map(|job| AgentJob::AudioAssets(Box::new(job))),
            Command::Candidates { operation } => {
                self.begin_candidates(operation).map(AgentJob::Candidates)
            }
            Command::Accompaniment {
                expected_revision,
                protected_clips,
                foreground_clips,
                operation,
            } => self
                .begin_accompaniment(
                    &expected_revision,
                    &protected_clips,
                    &foreground_clips,
                    operation,
                )
                .map(|job| AgentJob::Accompaniment(Box::new(job))),
            Command::Arrangement {
                operation:
                    crate::arrangement::Operation::Validate {
                        expected_revision,
                        changes,
                    },
            } => {
                if !crate::arrangement::enabled() {
                    return Err(
                        "Arrangement harness is disabled; opt in with AURIS_ARRANGEMENT_HARNESS=1"
                            .into(),
                    );
                }
                self.begin_arrangement(expected_revision, changes, &settings.plugin_paths)
                    .map(|job| AgentJob::Arrangement(Box::new(job)))
            }
            Command::Checkpoints {
                operation,
                expected_revision,
            } => self
                .begin_agent_checkpoints(operation, expected_revision)
                .map(|job| AgentJob::History(Box::new(job))),
            Command::ModulationSpectrum {
                section,
                instance,
                each_section,
                start_bar,
                bars,
                track,
                bands,
            } => {
                let selection = crate::modulation::Selection {
                    section,
                    instance,
                    start_bar,
                    bars,
                    each_section,
                    bands,
                };
                self.modulation_job(&selection, track)
                    .map(|job| AgentJob::Modulation(Box::new(job)))
            }
            command @ (Command::ListVoices { .. }
            | Command::SetSingerVoice { .. }
            | Command::Sing { .. }) => self
                .begin_agent_vocal(command, settings)
                .map(|job| AgentJob::Vocal(Box::new(job))),
            command => self
                .begin_agent_file_job(command)
                .map(|job| AgentJob::File(Box::new(job))),
        }
    }

    /// Validate and continue a worker result on the owning document thread.
    pub fn continue_agent_job(
        &mut self,
        result: AgentResult,
        cancelled: &AtomicBool,
    ) -> Result<AgentStep, String> {
        self.with_agent_history(|session| session.publish_agent_job(result, cancelled))
    }

    fn publish_agent_job(
        &mut self,
        result: AgentResult,
        cancelled: &AtomicBool,
    ) -> Result<AgentStep, String> {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Agent operation cancelled before publication".into());
        }
        match result {
            AgentResult::AudioAssets(result) => self
                .continue_audio_assets(*result, cancelled)
                .map(AgentStep::Done),
            AgentResult::Candidates(result) => self
                .continue_candidates(result, cancelled)
                .map(AgentStep::Done),
            AgentResult::Accompaniment(result) => self
                .continue_accompaniment(*result, cancelled)
                .map(AgentStep::Done),
            AgentResult::Arrangement(result) => self
                .continue_arrangement(*result, cancelled)
                .map(AgentStep::Done),
            AgentResult::History(result) => self
                .continue_agent_checkpoints(*result, cancelled)
                .map(AgentStep::Done),
            AgentResult::Vocal(result) => match self.continue_agent_vocal(result)? {
                VocalStep::Work(job) => Ok(AgentStep::Work(AgentJob::Vocal(job))),
                VocalStep::Done(text) => Ok(AgentStep::Done(text)),
            },
            AgentResult::File(result) => self
                .continue_agent_file_job(result, cancelled)
                .map(AgentStep::Done),
            // Read-only: the owner checked the revision before handing the result back. The
            // report travels whole; each door draws its picture with the toolbox's presenter.
            AgentResult::Modulation(report) => serde_json::to_string(&report)
                .map(AgentStep::Done)
                .map_err(|e| e.to_string()),
        }
    }
}
