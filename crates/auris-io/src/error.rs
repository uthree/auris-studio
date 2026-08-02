//! Errors produced by file import, export and project persistence.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Everything that can go wrong while reading or writing files.
///
/// Every variant carries enough context to be shown to the user directly: the failing path, or
/// the message the underlying library produced. `auris-io` never logs-and-swallows a failure,
/// because a silently truncated import or export is worse than a dialog.
#[derive(Debug, Error)]
pub enum IoError {
    /// The requested file does not exist, or is not readable.
    #[error("file not found: {0}")]
    FileNotFound(PathBuf),

    /// The container or codec is not one Symphonia was built to handle.
    #[error("unsupported audio format: {0}")]
    UnsupportedFormat(String),

    /// The file was recognised but its audio data could not be decoded.
    #[error("failed to decode audio: {0}")]
    Decode(String),

    /// Sample rate conversion failed, or could not be set up for the requested ratio.
    #[error("failed to resample audio: {0}")]
    Resample(String),

    /// Writing a WAV file failed.
    #[error("failed to write WAV file: {0}")]
    WavWrite(String),

    /// A project file could not be parsed as JSON, or a project could not be serialised.
    #[error("project JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The project file was written by a newer build of Auris Studio.
    #[error(
        "project format version {found} is newer than the supported version {supported}; \
         update Auris Studio to open this project"
    )]
    ProjectVersionMismatch {
        /// Version recorded in the file.
        found: u32,
        /// Newest version this build understands.
        supported: u32,
    },

    /// Any other filesystem failure, such as a permission or disk-full error.
    #[error("I/O error on {path}: {source}")]
    Filesystem {
        /// File the operation was working on.
        path: PathBuf,
        /// Underlying operating system error.
        source: std::io::Error,
    },
}

impl IoError {
    /// Classifies a filesystem error against the path it happened on.
    ///
    /// A missing file is by far the most common failure and deserves its own message, so it is
    /// split out from the generic case here rather than at every call site.
    pub(crate) fn from_fs(path: &Path, source: std::io::Error) -> Self {
        if source.kind() == std::io::ErrorKind::NotFound {
            IoError::FileNotFound(path.to_path_buf())
        } else {
            IoError::Filesystem {
                path: path.to_path_buf(),
                source,
            }
        }
    }
}

/// Result alias used throughout this crate.
pub type Result<T, E = IoError> = std::result::Result<T, E>;
