//! Bounded whole-file reads for formats whose parser needs an in-memory byte slice.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::{IoError, Result};

/// Reads at most `limit + 1` bytes from one already-open file handle.
///
/// Metadata rejects an obviously oversized file without allocating for it. The limited read is
/// still authoritative: the file may grow after metadata was observed, and checking only the
/// earlier length would reopen an unbounded `read_to_end` race.
pub(crate) fn read_with_limit(
    path: &Path,
    limit: usize,
    too_large: impl FnOnce(u64) -> IoError,
) -> Result<Vec<u8>> {
    read_with_limit_after_metadata(path, limit, || {}, too_large)
}

/// [`read_with_limit`] with a seam used to reproduce growth after the metadata check.
pub(crate) fn read_with_limit_after_metadata(
    path: &Path,
    limit: usize,
    after_metadata: impl FnOnce(),
    too_large: impl FnOnce(u64) -> IoError,
) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(|error| IoError::from_fs(path, error))?;
    let metadata_len = file
        .metadata()
        .map_err(|error| IoError::from_fs(path, error))?
        .len();
    let limit_u64 = u64::try_from(limit).unwrap_or(u64::MAX);
    if metadata_len > limit_u64 {
        return Err(too_large(metadata_len));
    }

    after_metadata();

    let initial_capacity = usize::try_from(metadata_len).unwrap_or(limit).min(limit);
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(initial_capacity).map_err(|error| {
        IoError::from_fs(
            path,
            std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                format!("could not reserve {initial_capacity} bytes for this file: {error}"),
            ),
        )
    })?;
    file.take(limit_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| IoError::from_fs(path, error))?;
    if bytes.len() > limit {
        return Err(too_large(bytes.len() as u64));
    }
    Ok(bytes)
}
