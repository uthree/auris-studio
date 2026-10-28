//! Native snapshots use the shared recovery lease/registry rules and a separate registry subtree.
//! A snapshot becomes discoverable only after owner-thread validation publishes its document.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use auris_core::pipeline::content::ContentLimits;
use auris_core::pipeline::document::NativeDocument;

use super::{NativeFileSession, NativeFileSessionError, copy_inside_assets};
use crate::native_edit::NativeDocumentEditor;
use crate::session::autosave::{
    self, AUTOSAVE_FILE, METADATA_FILE, RecoverySnapshot, SessionWorkspace,
};

pub(super) struct RecoveryState {
    pub(super) workspace: Arc<SessionWorkspace>,
    revision: u64,
}

/// An abandoned native snapshot discovered under the native recovery registry.
///
/// Recovery opens an unsaved copy; this path is never adopted as a permanent project path.
#[derive(Clone, Debug)]
pub struct NativeRecoverySnapshot {
    snapshot: RecoverySnapshot,
    registry: PathBuf,
}

impl NativeRecoverySnapshot {
    /// Snapshot title, when its bounded display metadata was readable.
    pub fn project_name(&self) -> Option<&str> {
        self.snapshot.project_name()
    }

    /// Original project path, for display only. Recovery never writes this path.
    pub fn source_document(&self) -> Option<&Path> {
        self.snapshot.source_document()
    }

    /// Time at which the snapshot document was last written.
    pub fn modified(&self) -> SystemTime {
        self.snapshot.modified()
    }

    /// Read-only document path for display or reveal-in-folder actions.
    pub fn document(&self) -> &Path {
        self.snapshot.document()
    }
}

/// Detached native autosave work. Run on a worker, then call `finish_autosave` on its owner.
pub struct NativeAutosaveJob {
    owner: Arc<()>,
    generation: u64,
    revision: u64,
    original_path: Option<PathBuf>,
    original_folder: Option<PathBuf>,
    document: NativeDocument,
    limits: ContentLimits,
    registries: Vec<PathBuf>,
    // A recovered session may be closed while this detached job still reads its files.
    _source_workspace: Option<Arc<SessionWorkspace>>,
}

/// A complete private snapshot awaiting the owner's revision and transaction checks.
pub struct NativeAutosaveResult {
    owner: Arc<()>,
    generation: u64,
    revision: u64,
    original_path: Option<PathBuf>,
    original_folder: Option<PathBuf>,
    document: auris_io::StagedNativeDocument,
    metadata: auris_io::StagedFile,
    // Close staged-file handles before deleting their workspace on rejected publication.
    workspace: Arc<SessionWorkspace>,
}

/// Loads a native recovery copy without changing any live document or the abandoned snapshot.
pub struct NativeRecoveryJob {
    snapshot: NativeRecoverySnapshot,
    limits: ContentLimits,
}

fn recovery_error(error: impl std::fmt::Display) -> NativeFileSessionError {
    NativeFileSessionError::Recovery(error.to_string())
}

fn registries() -> Vec<PathBuf> {
    // Ordinary Project recovery discovery is non-recursive. Keep the native format out of its
    // chooser until that frontend has switched its document authority.
    autosave::known_recovery_roots()
        .into_iter()
        .map(|root| root.join("native"))
        .collect()
}

impl NativeFileSession {
    pub(super) fn require_permanent_destination(
        &self,
        path: &Path,
    ) -> Result<(), NativeFileSessionError> {
        for workspace in self
            .asset_workspace
            .iter()
            .chain(self.recovery.iter().map(|state| &state.workspace))
        {
            let root = std::fs::canonicalize(workspace.path()).map_err(recovery_error)?;
            let absolute = std::path::absolute(path).map_err(recovery_error)?;
            // A Save As folder may not exist yet. Its closest existing ancestor still identifies
            // whether publication would place a permanent project inside disposable scratch.
            for ancestor in absolute.ancestors() {
                match std::fs::canonicalize(ancestor) {
                    Ok(resolved) => {
                        if resolved.starts_with(&root) {
                            return Err(recovery_error(
                                "choose a permanent project folder outside recovery storage",
                            ));
                        }
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(recovery_error(error)),
                }
            }
        }
        Ok(())
    }

    /// Captures a dirty, idle document for native autosave. The caller owns timer scheduling.
    ///
    /// Returns `None` if this exact revision already has a recovery snapshot or is saved.
    /// Creating the job performs no filesystem IO; dropping a job/result cannot publish it.
    pub fn begin_autosave(&mut self) -> Result<Option<NativeAutosaveJob>, NativeFileSessionError> {
        self.begin_autosave_in_roots(registries())
    }

    #[cfg(test)]
    fn begin_autosave_in(
        &mut self,
        registry: PathBuf,
    ) -> Result<Option<NativeAutosaveJob>, NativeFileSessionError> {
        self.begin_autosave_in_roots(vec![registry])
    }

    fn begin_autosave_in_roots(
        &mut self,
        registries: Vec<PathBuf>,
    ) -> Result<Option<NativeAutosaveJob>, NativeFileSessionError> {
        if self.editor.transaction_owner().is_some() {
            return Err(NativeFileSessionError::ActiveEdit);
        }
        let revision = self.editor.revision().document;
        if !self.editor.is_dirty()
            || self
                .recovery
                .as_ref()
                .is_some_and(|state| state.revision == revision)
        {
            return Ok(None);
        }
        self.recovery_generation = self
            .recovery_generation
            .checked_add(1)
            .ok_or(NativeFileSessionError::StaleSave)?;
        Ok(Some(NativeAutosaveJob {
            owner: Arc::clone(&self.owner),
            generation: self.recovery_generation,
            revision,
            original_path: self.path.clone(),
            original_folder: self.asset_folder.clone(),
            document: self.editor.document().clone(),
            limits: self.limits,
            registries,
            _source_workspace: self.asset_workspace.clone(),
        }))
    }

    /// Publishes a prepared recovery snapshot only while its native owner is unchanged.
    ///
    /// Autosave does not acknowledge the permanent save baseline or clear dirty state.
    pub fn finish_autosave(
        &mut self,
        result: NativeAutosaveResult,
        cancelled: &AtomicBool,
    ) -> Result<(), NativeFileSessionError> {
        if cancelled.load(Ordering::Relaxed)
            || !self.editor.is_dirty()
            || !Arc::ptr_eq(&self.owner, &result.owner)
            || result.generation != self.recovery_generation
            || result.revision != self.editor.revision().document
            || result.original_path != self.path
            || result.original_folder != self.asset_folder
            || self.editor.transaction_owner().is_some()
        {
            return Err(NativeFileSessionError::StaleSave);
        }
        result.metadata.publish().map_err(recovery_error)?;
        auris_io::publish_native_new(result.document)?;
        self.recovery = Some(RecoveryState {
            workspace: result.workspace,
            revision: result.revision,
        });
        Ok(())
    }

    /// Lists abandoned native snapshots; live sessions retain an exclusive lease and are hidden.
    pub fn recovery_snapshots() -> Result<Vec<NativeRecoverySnapshot>, NativeFileSessionError> {
        let mut snapshots = Vec::new();
        let mut first_error = None;
        let mut readable = false;
        for root in registries() {
            match snapshots_in(&root) {
                Ok(found) => {
                    readable = true;
                    snapshots.extend(found);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if !readable && let Some(error) = first_error {
            return Err(error);
        }
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.modified()));
        Ok(snapshots)
    }

    /// Current leased native autosave path, if one has been published.
    pub fn autosave_path(&self) -> Option<PathBuf> {
        self.recovery
            .as_ref()
            .map(|state| state.workspace.path().join(AUTOSAVE_FILE))
    }

    /// Captures a native recovery request without IO or a live Session mutation.
    pub fn begin_recovery(
        snapshot: &NativeRecoverySnapshot,
        limits: ContentLimits,
    ) -> NativeRecoveryJob {
        NativeRecoveryJob {
            snapshot: snapshot.clone(),
            limits,
        }
    }

    /// Discards one abandoned native recovery workspace using the common lease/quarantine rules.
    pub fn discard_recovery(
        snapshot: &NativeRecoverySnapshot,
    ) -> Result<(), NativeFileSessionError> {
        crate::Session::discard_recovery(&snapshot.snapshot).map_err(recovery_error)
    }
}

fn snapshots_in(root: &Path) -> Result<Vec<NativeRecoverySnapshot>, NativeFileSessionError> {
    Ok(autosave::recovery_snapshots_in(root)
        .map_err(recovery_error)?
        .into_iter()
        .map(|snapshot| NativeRecoverySnapshot {
            snapshot,
            registry: root.to_path_buf(),
        })
        .collect())
}

fn create_workspace(roots: &[PathBuf]) -> Result<SessionWorkspace, NativeFileSessionError> {
    let mut last_error = recovery_error("no native recovery registry configured");
    for root in roots {
        match autosave::create_recovery_workspace_in(root) {
            Ok(workspace) => return Ok(workspace),
            Err(error) => last_error = recovery_error(error),
        }
    }
    Err(last_error)
}

impl NativeAutosaveJob {
    /// Copies inside assets and stages a snapshot in a leased private workspace.
    /// Cancellation is checked between filesystem stages; no partial snapshot is discoverable.
    pub fn run(
        self,
        cancelled: &AtomicBool,
    ) -> Result<Option<NativeAutosaveResult>, NativeFileSessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let workspace = Arc::new(create_workspace(&self.registries)?);
        copy_inside_assets(
            &self.document,
            self.original_folder.as_deref(),
            Some(workspace.path()),
        )?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let document = auris_io::stage_native(
            &workspace.path().join(AUTOSAVE_FILE),
            &self.document,
            self.limits,
        )?;
        let bytes = autosave::recovery_metadata_for(
            &self.document.context.name,
            self.original_path.clone(),
        )
        .map_err(recovery_error)?;
        let metadata = auris_io::stage_file_bytes(&workspace.path().join(METADATA_FILE), &bytes)
            .map_err(recovery_error)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(NativeAutosaveResult {
            owner: self.owner,
            generation: self.generation,
            revision: self.revision,
            original_path: self.original_path,
            original_folder: self.original_folder,
            workspace,
            document,
            metadata,
        }))
    }
}

impl NativeRecoveryJob {
    /// Reopens an unsaved native authority with its own copied assets and recovery lease.
    /// The original snapshot remains available until explicitly discarded.
    pub fn run(
        self,
        cancelled: &AtomicBool,
    ) -> Result<Option<NativeFileSession>, NativeFileSessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let _lease =
            autosave::acquire_snapshot_lease(&self.snapshot.snapshot).map_err(recovery_error)?;
        let document = auris_io::load_native(self.snapshot.document(), self.limits)?;
        autosave::validate_snapshot(&self.snapshot.snapshot).map_err(recovery_error)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let workspace = Arc::new(
            autosave::create_recovery_workspace_in(&self.snapshot.registry)
                .map_err(recovery_error)?,
        );
        copy_inside_assets(
            &document,
            self.snapshot.document().parent(),
            Some(workspace.path()),
        )?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let staged = auris_io::stage_native(
            &workspace.path().join(AUTOSAVE_FILE),
            &document,
            self.limits,
        )?;
        let bytes = autosave::recovery_metadata_for(
            &document.context.name,
            self.snapshot.source_document().map(Path::to_path_buf),
        )
        .map_err(recovery_error)?;
        let metadata = auris_io::stage_file_bytes(&workspace.path().join(METADATA_FILE), &bytes)
            .map_err(recovery_error)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        metadata.publish().map_err(recovery_error)?;
        auris_io::publish_native_new(staged)?;
        let mut session = NativeFileSession::from_editor(
            NativeDocumentEditor::new_unsaved(document, self.limits)?,
            self.limits,
            Some(workspace.path().to_path_buf()),
        );
        session.asset_workspace = Some(Arc::clone(&workspace));
        session.recovery = Some(RecoveryState {
            workspace,
            revision: 0,
        });
        Ok(Some(session))
    }
}

#[cfg(test)]
mod tests;
