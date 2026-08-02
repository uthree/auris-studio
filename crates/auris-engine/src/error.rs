//! Errors produced while opening audio hardware or rendering a project.

use thiserror::Error;

/// Everything that can go wrong in the engine.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The host reported no default output device.
    #[error("no default audio output device is available")]
    NoOutputDevice,

    /// The device's sample format is not one the engine knows how to write.
    #[error("unsupported output sample format `{0}`")]
    UnsupportedSampleFormat(String),

    /// The audio backend refused an operation.
    #[error("audio backend error: {0}")]
    Backend(#[from] cpal::Error),

    /// A core data structure rejected an operation.
    #[error(transparent)]
    Core(#[from] auris_core::CoreError),

    /// An offline render was asked for a range whose end precedes its start.
    #[error("invalid render range: start {start} is past end {end}")]
    InvalidRange {
        /// First frame of the requested range.
        start: u64,
        /// One past the last frame of the requested range.
        end: u64,
    },

    /// A sample rate that is not finite and positive was requested.
    #[error("invalid sample rate {0}")]
    InvalidSampleRate(f64),

    /// An offline render covered more frames than the engine is willing to attempt.
    #[error("render span of {frames} frames exceeds the {limit} frame limit")]
    RenderTooLong {
        /// Frames the requested render would have covered.
        frames: u64,
        /// Largest span the engine will attempt.
        limit: u64,
    },

    /// The command queue was full; the caller should retry on the next UI frame.
    #[error("the engine command queue is full")]
    CommandQueueFull,

    /// The audio thread is gone, so commands can no longer be delivered.
    #[error("the audio engine is not running")]
    NotRunning,
}
