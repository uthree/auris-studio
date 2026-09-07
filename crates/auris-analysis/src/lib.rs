//! Offline music recognition, with deterministic DSP and optional local model inference.
//!
//! Scores describe agreement with a template or periodicity; they are not probabilities.
//! The caller owns decoding, document editing and worker scheduling.

#![warn(missing_docs)]

pub mod audio;
pub mod chords;
pub mod instruments;
pub mod mixture;
mod pitch;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// A rejected request or an interrupted worker.
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    /// Input or options cannot be analyzed within the supported bounds.
    #[error("music analysis: {0}")]
    Invalid(&'static str),
    /// The caller cancelled the job.
    #[error("music analysis cancelled")]
    Cancelled,
    /// A local model could not be loaded or returned incompatible output.
    #[error("music analysis model: {0}")]
    Model(String),
}

/// Shareable cancellation and progress for one analysis job.
#[derive(Clone, Debug, Default)]
pub struct AnalysisControl {
    cancelled: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
}

impl AnalysisControl {
    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    /// Requests cancellation at the next bounded processing step.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    /// Completed work as a fraction in 0..=1.
    pub fn progress(&self) -> f32 {
        self.progress.load(Ordering::Relaxed) as f32 / 1000.0
    }
    /// Publishes progress from an external worker and observes cancellation.
    pub fn report_progress(&self, fraction: f32) -> Result<(), AnalysisError> {
        self.check(fraction)
    }
    pub(crate) fn check(&self, fraction: f32) -> Result<(), AnalysisError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(AnalysisError::Cancelled);
        }
        self.progress.store(
            (fraction.clamp(0.0, 1.0) * 1000.0) as usize,
            Ordering::Relaxed,
        );
        Ok(())
    }
}
