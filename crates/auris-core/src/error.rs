//! Error type shared by the core data structures.

use thiserror::Error;

/// Errors produced while manipulating core data structures.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A planar buffer was constructed from channels of differing lengths.
    #[error("channel {channel} has {found} frames but the buffer has {expected}")]
    RaggedChannels {
        /// Index of the offending channel.
        channel: usize,
        /// Length that channel actually had.
        found: usize,
        /// Length every channel was expected to have.
        expected: usize,
    },

    /// An operation required at least one channel.
    #[error("audio buffer must have at least one channel")]
    NoChannels,

    /// Two buffers that had to line up did not.
    #[error("buffer layout mismatch: {0}")]
    LayoutMismatch(String),

    /// A plugin id was not present in the registry.
    #[error("unknown plugin id `{0}`")]
    UnknownPlugin(String),

    /// A track/clip id referenced an object that no longer exists.
    #[error("unknown {kind} id {id}")]
    UnknownId {
        /// What kind of object was being looked up ("track", "clip", ...).
        kind: &'static str,
        /// The numeric id that failed to resolve.
        id: u64,
    },

    /// A tempo map was built without any tempo points, or with an invalid one.
    #[error("invalid tempo map: {0}")]
    InvalidTempoMap(String),

    /// Text that was meant to be a time signature was not one.
    #[error("`{0}` is not a time signature like 4/4")]
    InvalidTimeSignature(String),

    /// An automation lane was built with no points, or with one that is not a real value at a
    /// real position.
    #[error("invalid automation lane: {0}")]
    InvalidAutomationLane(String),
}

/// Result alias used throughout the core crate.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;
