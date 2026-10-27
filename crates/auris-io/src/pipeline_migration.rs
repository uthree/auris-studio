//! Captured-byte migration from the legacy project format to the native pipeline document.

use std::path::Path;

use auris_core::pipeline::content::ContentLimits;
use auris_core::pipeline::document::NativeDocument;
use thiserror::Error;

use crate::project_file::{
    LegacyDecodeReport, MAX_PROJECT_FILE_BYTES, decode_legacy_bytes, read_project_source,
};
use crate::{LegacyBackupOutcome, PipelineConversionError, ProjectBackupError};

/// Repair facts observed while decoding the captured legacy bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineMigrationReport {
    /// Format version found in the source bytes, before loader normalization.
    pub source_format_version: u32,
    /// Whether the legacy object ID counter was repaired in memory.
    pub id_counter_repaired: bool,
    /// Whether legacy clip pitch-bend or controller points were sorted in memory.
    pub curve_order_repaired: bool,
    /// Whether invalid legacy routing was repaired in memory.
    pub routing_repaired: bool,
}

impl From<LegacyDecodeReport> for PipelineMigrationReport {
    fn from(report: LegacyDecodeReport) -> Self {
        Self {
            source_format_version: report.source_format_version,
            id_counter_repaired: report.id_counter_repaired,
            curve_order_repaired: report.curve_order_repaired,
            routing_repaired: report.routing_repaired,
        }
    }
}

/// A converted native candidate retaining the exact source bytes for backup.
///
/// The candidate has no publication method. Callers must explicitly choose where and how the
/// native document is persisted after M2 activation and source-change checks.
#[derive(Debug)]
pub struct NativeMigrationCandidate {
    document: NativeDocument,
    report: PipelineMigrationReport,
    source_bytes: Vec<u8>,
}

impl NativeMigrationCandidate {
    /// The converted native document.
    pub fn document(&self) -> &NativeDocument {
        &self.document
    }

    /// Repair and source-format facts from the captured legacy bytes.
    pub fn report(&self) -> PipelineMigrationReport {
        self.report
    }

    /// Returns the captured source bytes without rereading the source path.
    pub fn source_bytes(&self) -> &[u8] {
        &self.source_bytes
    }

    /// Preserves the captured source bytes at an exact-byte, no-clobber backup destination.
    pub fn preserve_backup(
        &self,
        destination: &Path,
    ) -> Result<LegacyBackupOutcome, ProjectBackupError> {
        crate::preserve_legacy_project_backup(&self.source_bytes, destination)
    }

    /// Compares the source path with the captured bytes using one bounded read.
    pub fn source_matches(&self, path: &Path) -> crate::Result<bool> {
        Ok(read_project_source(path, MAX_PROJECT_FILE_BYTES)? == self.source_bytes)
    }
}

/// Failure while capturing, decoding or converting a legacy project.
#[derive(Debug, Error)]
pub enum PipelineMigrationError {
    /// Source capture or decoding failed.
    #[error(transparent)]
    Io(#[from] crate::IoError),
    /// Native conversion rejected the candidate without publishing anything.
    #[error(transparent)]
    Conversion(#[from] PipelineConversionError),
}

/// Captures and converts one legacy project, reading the source path exactly once.
pub fn load_native_candidate(
    path: &Path,
    limits: ContentLimits,
) -> Result<NativeMigrationCandidate, PipelineMigrationError> {
    let source_bytes = read_project_source(path, MAX_PROJECT_FILE_BYTES)?;
    let (project, report) = decode_legacy_bytes(&source_bytes, path)?;
    let mut document = crate::project_to_native(&project, limits)?;
    document.context.source_format_version = report.source_format_version;
    Ok(NativeMigrationCandidate {
        document,
        report: report.into(),
        source_bytes,
    })
}
