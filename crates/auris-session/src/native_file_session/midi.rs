//! Native MIDI import jobs and their revision-bound session handoff.

use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

use auris_core::Ticks;

use crate::midi_import::PreparedMidiImport;
use crate::native_edit::{NativeEdit, NativeEditOwner, NativeEditReceipt};
use crate::session::MidiReport;

use super::{NativeFileSession, NativeFileSessionError};

/// A worker-safe MIDI import captured from one native editor revision.
pub struct NativeMidiImportJob {
    owner: Arc<()>,
    revision: u64,
    path: PathBuf,
    at: Ticks,
    edit_owner: NativeEditOwner,
    fallback: String,
    drums: String,
}

/// A parsed MIDI import waiting for the short native-session handoff.
pub struct NativeMidiImportResult {
    owner: Arc<()>,
    revision: u64,
    edit_owner: NativeEditOwner,
    prepared: PreparedMidiImport,
    at: Ticks,
}

/// The report and atomic native edit produced by a successful MIDI append.
pub struct NativeMidiImportReceipt {
    /// MIDI file statistics.
    pub report: MidiReport,
    /// The single undoable native edit receipt.
    pub edit: NativeEditReceipt,
}

impl NativeMidiImportJob {
    /// Parses the MIDI file without touching the native editor.
    pub fn run(
        self,
        cancelled: &AtomicBool,
    ) -> Result<Option<NativeMidiImportResult>, NativeFileSessionError> {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        let prepared = PreparedMidiImport::read(&self.path, &self.fallback, &self.drums, cancelled)
            .map_err(|error| NativeFileSessionError::MidiImport(error.to_string()))?;
        let Some(prepared) = prepared else {
            return Ok(None);
        };
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(NativeMidiImportResult {
            owner: self.owner,
            revision: self.revision,
            edit_owner: self.edit_owner,
            prepared,
            at: self.at,
        }))
    }
}

impl NativeFileSession {
    /// Captures a revision-bound worker job that appends MIDI clips at `at`.
    pub fn begin_midi_append(
        &self,
        path: &Path,
        at: Ticks,
        edit_owner: NativeEditOwner,
        fallback: &str,
        drums: &str,
    ) -> Result<NativeMidiImportJob, NativeFileSessionError> {
        if self.editor.transaction_owner().is_some() {
            return Err(NativeFileSessionError::ActiveEdit);
        }
        if at.raw() < 0 {
            return Err(NativeFileSessionError::MidiImport(
                "MIDI append offset cannot be negative".into(),
            ));
        }
        if fallback.trim().is_empty() || drums.trim().is_empty() {
            return Err(NativeFileSessionError::MidiImport(
                "MIDI import requires fallback and drum instruments".into(),
            ));
        }
        Ok(NativeMidiImportJob {
            owner: Arc::clone(&self.owner),
            revision: self.editor.revision().document,
            path: path.to_path_buf(),
            at,
            edit_owner,
            fallback: fallback.to_owned(),
            drums: drums.to_owned(),
        })
    }

    /// Commits a parsed MIDI append as one native edit and one undo step.
    pub fn finish_midi_append(
        &mut self,
        result: NativeMidiImportResult,
    ) -> Result<NativeMidiImportReceipt, NativeFileSessionError> {
        if !Arc::ptr_eq(&self.owner, &result.owner)
            || result.revision != self.editor.revision().document
        {
            return Err(NativeFileSessionError::StaleMidiImport);
        }
        if self.editor.transaction_owner().is_some() {
            return Err(NativeFileSessionError::ActiveEdit);
        }
        let report = result.prepared.report();
        let edit = NativeEdit::ImportMidi {
            tracks: result.prepared.tracks,
            at: result.at,
        };
        let receipt = self.editor.apply(
            result.revision,
            result.edit_owner,
            crate::Edit::ExternalChanges,
            &[edit],
        )?;
        Ok(NativeMidiImportReceipt {
            report,
            edit: receipt,
        })
    }
}
