//! Exact-byte preservation for legacy project documents.
//!
//! Migration uses this small primitive before it publishes an upgraded project.  It deliberately
//! accepts the bytes captured by the caller rather than a [`Project`](auris_core::Project): a
//! backup is a recovery artifact, so whitespace and unknown legacy fields must survive unchanged.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::error::IoError;
use crate::project_file::{MAX_PROJECT_FILE_BYTES, read_project_source, stage_file_bytes};

/// The result of preserving a legacy project document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyBackupOutcome {
    /// The captured bytes were published at the requested destination.
    Created,
    /// The destination already contained exactly the captured bytes.
    AlreadyPresent,
}

/// Failure while preserving an exact-byte legacy project backup.
#[derive(Debug, Error)]
pub enum ProjectBackupError {
    /// The captured source is larger than the project-file memory bound.
    #[error("legacy project backup is too large: {observed} bytes; the limit is {limit} bytes")]
    TooLarge {
        /// Number of bytes supplied by the caller.
        observed: usize,
        /// Maximum number of bytes this operation accepts.
        limit: usize,
    },

    /// A destination file exists, but its bytes differ from the captured source.
    #[error("legacy project backup already exists with different bytes: {path}")]
    ExistingBytesDiffer {
        /// Destination whose contents did not match.
        path: PathBuf,
    },

    /// A destination symlink was encountered.  It is never treated as an idempotent backup.
    #[error("legacy project backup destination is a symbolic link: {path}")]
    ExistingSymlink {
        /// Destination symbolic link.
        path: PathBuf,
    },

    /// A directory occupies the requested destination.
    #[error("legacy project backup destination is a directory: {path}")]
    ExistingDirectory {
        /// Destination directory.
        path: PathBuf,
    },

    /// The destination is not a regular file that can serve as a document backup.
    #[error("legacy project backup destination is not a regular file: {path}")]
    ExistingSpecialFile {
        /// Destination with an unsupported file type.
        path: PathBuf,
    },

    /// The filesystem rejected staging or verification.
    #[error(transparent)]
    Io(#[from] IoError),
}

/// Preserves the caller's captured legacy bytes without serialising or replacing a destination.
///
/// The bytes must have been captured from the legacy source before migration starts.  This
/// function does not re-check that source, and therefore does not by itself prove that the source
/// remained unchanged between capture and publication; the caller must perform that source-change
/// check as part of its migration transaction.  Publication is no-clobber and safe against a
/// competing writer claiming the destination.  A pre-existing regular file is accepted only when
/// a bounded read proves that every byte is identical.
pub fn preserve_legacy_project_backup(
    bytes: &[u8],
    destination: &Path,
) -> std::result::Result<LegacyBackupOutcome, ProjectBackupError> {
    if bytes.len() > MAX_PROJECT_FILE_BYTES {
        return Err(ProjectBackupError::TooLarge {
            observed: bytes.len(),
            limit: MAX_PROJECT_FILE_BYTES,
        });
    }

    let staged = stage_file_bytes(destination, bytes)?;
    match staged.publish_noclobber() {
        Ok(()) => Ok(LegacyBackupOutcome::Created),
        Err(IoError::ExportDestinationExists(_)) => verify_existing_backup(bytes, destination),
        Err(error) => Err(ProjectBackupError::Io(error)),
    }
}

fn verify_existing_backup(
    expected: &[u8],
    destination: &Path,
) -> std::result::Result<LegacyBackupOutcome, ProjectBackupError> {
    let metadata = std::fs::symlink_metadata(destination)
        .map_err(|error| ProjectBackupError::Io(IoError::from_fs(destination, error)))?;
    if metadata.file_type().is_symlink() {
        return Err(ProjectBackupError::ExistingSymlink {
            path: destination.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        return Err(ProjectBackupError::ExistingDirectory {
            path: destination.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(ProjectBackupError::ExistingSpecialFile {
            path: destination.to_path_buf(),
        });
    }

    let actual = read_project_source(destination, MAX_PROJECT_FILE_BYTES)?;
    if actual == expected {
        Ok(LegacyBackupOutcome::AlreadyPresent)
    } else {
        Err(ProjectBackupError::ExistingBytesDiffer {
            path: destination.to_path_buf(),
        })
    }
}
