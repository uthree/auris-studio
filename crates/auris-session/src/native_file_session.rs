//! Persistence authority for a native pipeline document.
//!
//! This owner deliberately contains no legacy [`auris_core::Project`]. It is the small session
//! boundary used while the ordinary Session is being migrated; callers must choose it explicitly.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_core::AssetPath;
use auris_core::pipeline::content::ContentLimits;
use auris_core::pipeline::document::NativeDocument;
use auris_io::{
    NativeFileError, NativeMigrationCandidate, NativeResult, load_native, stage_native,
};

use crate::native_edit::{NativeDocumentEditor, NativeEditError};

mod recording;
pub use recording::NativeRecordingFile;
mod recovery;
pub use recovery::{
    NativeAutosaveJob, NativeAutosaveResult, NativeRecoveryJob, NativeRecoverySnapshot,
};

/// Errors from the native persistence authority.
#[derive(Debug, thiserror::Error)]
pub enum NativeFileSessionError {
    /// Native file IO or validation failed.
    #[error(transparent)]
    Io(#[from] NativeFileError),
    /// Native editor validation rejected the operation.
    #[error(transparent)]
    Edit(#[from] NativeEditError),
    /// Legacy migration capture failed.
    #[error("native legacy migration failed: {0}")]
    Migration(String),
    /// An inside-project asset could not be copied into the Save As project.
    #[error("native Save As could not collect project asset: {0}")]
    AssetCopy(String),
    /// The saved job no longer belongs to the current editor state.
    #[error("native save job is stale or out of order")]
    StaleSave,
    /// The editor has an active human or agent gesture.
    #[error("finish the active native edit before saving")]
    ActiveEdit,
    /// Recovery storage or lease validation failed.
    #[error("native recovery failed: {0}")]
    Recovery(String),
}

/// A native document authority with explicit editor and file identity.
pub struct NativeFileSession {
    editor: NativeDocumentEditor,
    path: Option<PathBuf>,
    asset_folder: Option<PathBuf>,
    limits: ContentLimits,
    /// Per-session identity; registry identity is deliberately not used for save ownership.
    owner: Arc<()>,
    /// Exact bytes acknowledged at open or the last successful publication.
    disk_bytes: Option<Vec<u8>>,
    save_generation: u64,
    legacy_source: Option<NativeMigrationCandidate>,
    recovery: Option<recovery::RecoveryState>,
    recovery_generation: u64,
    /// Keeps recovered inside-project files alive until Save As copies them out.
    asset_workspace: Option<Arc<crate::session::autosave::SessionWorkspace>>,
}

impl NativeFileSession {
    /// Creates an unsaved native document authority.
    pub fn new(
        document: NativeDocument,
        limits: ContentLimits,
    ) -> Result<Self, NativeFileSessionError> {
        Ok(Self {
            editor: NativeDocumentEditor::new(document, limits)?,
            path: None,
            asset_folder: None,
            limits,
            owner: Arc::new(()),
            disk_bytes: None,
            save_generation: 0,
            legacy_source: None,
            recovery: None,
            recovery_generation: 0,
            asset_workspace: None,
        })
    }

    /// Transfers a captured editor without resetting its history or saved-state comparison.
    pub(crate) fn from_editor(
        editor: NativeDocumentEditor,
        limits: ContentLimits,
        asset_folder: Option<PathBuf>,
    ) -> Self {
        Self {
            editor,
            path: None,
            asset_folder,
            limits,
            owner: Arc::new(()),
            disk_bytes: None,
            save_generation: 0,
            legacy_source: None,
            recovery: None,
            recovery_generation: 0,
            asset_workspace: None,
        }
    }

    /// Opens a native file, or captures a legacy file exactly once for explicit migration.
    pub fn open(path: &Path, limits: ContentLimits) -> Result<Self, NativeFileSessionError> {
        match auris_io::load_native_with_source(path, limits) {
            Ok((document, disk_bytes)) => {
                let mut session = Self::new(document, limits)?;
                session.path = Some(path.to_path_buf());
                session.asset_folder = path.parent().map(Path::to_path_buf);
                session.disk_bytes = Some(disk_bytes);
                Ok(session)
            }
            Err(NativeFileError::VersionMismatch { .. }) => {
                let candidate = auris_io::load_native_candidate(path, limits)
                    .map_err(|error| NativeFileSessionError::Migration(error.to_string()))?;
                let document = candidate.document().clone();
                let mut session = Self::new(document, limits)?;
                session.path = Some(path.to_path_buf());
                session.asset_folder = path.parent().map(Path::to_path_buf);
                session.disk_bytes = Some(candidate.source_bytes().to_vec());
                session.legacy_source = Some(candidate);
                Ok(session)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Reads the native editor authority.
    pub fn editor(&self) -> &NativeDocumentEditor {
        &self.editor
    }

    /// Mutates the native editor authority without converting a legacy project.
    pub fn editor_mut(&mut self) -> &mut NativeDocumentEditor {
        &mut self.editor
    }

    /// Current native file path, if the document has been saved.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Folder against which inside-project assets are resolved, including unsaved captures.
    pub fn asset_folder(&self) -> Option<&Path> {
        self.asset_folder.as_deref()
    }

    /// Captures an immutable in-place save job.
    pub fn begin_save(&mut self) -> Result<NativeSaveJob, NativeFileSessionError> {
        let path = self.path.clone().ok_or(NativeFileSessionError::StaleSave)?;
        self.begin_save_at(path)
    }

    /// Captures an immutable Save As job after collecting inside-project assets when needed.
    pub fn begin_save_as(
        &mut self,
        chosen: &Path,
    ) -> Result<NativeSaveJob, NativeFileSessionError> {
        let path = auris_io::document_in_folder(chosen);
        self.begin_save_at(path)
    }

    fn begin_save_at(&mut self, path: PathBuf) -> Result<NativeSaveJob, NativeFileSessionError> {
        if self.editor.transaction_owner().is_some() {
            return Err(NativeFileSessionError::ActiveEdit);
        }
        self.require_permanent_destination(&path)?;
        self.save_generation = self
            .save_generation
            .checked_add(1)
            .ok_or(NativeFileSessionError::StaleSave)?;
        let in_place = self.path.as_ref() == Some(&path);
        Ok(NativeSaveJob {
            owner: Arc::clone(&self.owner),
            generation: self.save_generation,
            revision: self.editor.revision().document,
            path,
            original_path: self.path.clone(),
            document: self.editor.document().clone(),
            limits: self.limits,
            legacy: in_place.then(|| self.legacy_source.clone()).flatten(),
            expected: in_place.then(|| self.disk_bytes.clone()).flatten(),
            original_folder: self.asset_folder.clone(),
        })
    }

    /// Finishes a prepared save only when it still belongs to this editor.
    pub fn finish_save(&mut self, result: NativeSaveResult) -> Result<(), NativeFileSessionError> {
        if !Arc::ptr_eq(&self.owner, &result.owner)
            || result.generation != self.save_generation
            || result.revision != self.editor.revision().document
            || result.original_path != self.path
            || self.editor.transaction_owner().is_some()
        {
            return Err(NativeFileSessionError::StaleSave);
        }
        let revision = result.revision;
        let path = result.path.clone();
        let disk_bytes = result.staged.encoded_bytes()?;
        result.publish()?;
        self.editor.acknowledge_save(revision)?;
        self.asset_folder = path.parent().map(Path::to_path_buf);
        self.path = Some(path);
        self.disk_bytes = Some(disk_bytes);
        self.legacy_source = None;
        self.recovery = None;
        self.asset_workspace = None;
        Ok(())
    }

    /// Creates a no-clobber native checkpoint beside the current project.
    pub fn create_checkpoint(&self, name: &str) -> Result<PathBuf, NativeFileSessionError> {
        if self.editor.transaction_owner().is_some() {
            return Err(NativeFileSessionError::ActiveEdit);
        }
        let folder = self
            .asset_folder
            .as_deref()
            .ok_or(NativeFileSessionError::StaleSave)?;
        if name.is_empty()
            || name.len() > 80
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(NativeFileSessionError::StaleSave);
        }
        let directory = folder.join(".auris-history");
        std::fs::create_dir_all(&directory).map_err(|error| {
            NativeFileSessionError::Io(NativeFileError::Io(auris_io::IoError::from_fs(
                &directory, error,
            )))
        })?;
        let path = directory.join(format!("checkpoint-{name}.auris"));
        let staged = stage_native(&path, self.editor.document(), self.limits)?;
        auris_io::publish_native_new(staged)?;
        Ok(path)
    }

    /// Restores a native checkpoint as one undoable edit after checking revision and owner.
    pub fn restore_checkpoint(
        &mut self,
        path: &Path,
        expected_revision: u64,
        owner: crate::native_edit::NativeEditOwner,
    ) -> Result<(), NativeFileSessionError> {
        if self.editor.transaction_owner().is_some() {
            return Err(NativeFileSessionError::ActiveEdit);
        }
        let directory = self
            .asset_folder
            .as_deref()
            .ok_or(NativeFileSessionError::StaleSave)?
            .join(".auris-history");
        let expected = std::fs::canonicalize(&directory)
            .map_err(|error| NativeFileSessionError::Migration(error.to_string()))?;
        let actual = std::fs::canonicalize(path)
            .map_err(|error| NativeFileSessionError::Migration(error.to_string()))?;
        if actual.parent() != Some(expected.as_path()) {
            return Err(NativeFileSessionError::Migration(
                "checkpoint belongs to a different asset folder".into(),
            ));
        }
        let document = load_native(path, self.limits)?;
        self.editor.replace_snapshot(
            expected_revision,
            owner,
            crate::Edit::ExternalChanges,
            document,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod asset_copy_tests;
#[cfg(test)]
mod tests;

/// An immutable worker-side native save request.
pub struct NativeSaveJob {
    owner: Arc<()>,
    generation: u64,
    revision: u64,
    path: PathBuf,
    original_path: Option<PathBuf>,
    document: NativeDocument,
    limits: ContentLimits,
    legacy: Option<NativeMigrationCandidate>,
    expected: Option<Vec<u8>>,
    original_folder: Option<PathBuf>,
}

/// A staged native save awaiting the owner-thread commit check.
pub struct NativeSaveResult {
    owner: Arc<()>,
    generation: u64,
    revision: u64,
    path: PathBuf,
    original_path: Option<PathBuf>,
    staged: auris_io::StagedNativeDocument,
    document: NativeDocument,
    limits: ContentLimits,
    legacy: Option<NativeMigrationCandidate>,
    expected: Option<Vec<u8>>,
    original_folder: Option<PathBuf>,
}

impl NativeSaveJob {
    /// Stages the native document without publishing it.
    pub fn run(self) -> Result<NativeSaveResult, NativeFileSessionError> {
        let folder = self
            .path
            .parent()
            .ok_or(NativeFileSessionError::StaleSave)?;
        std::fs::create_dir_all(folder)
            .map_err(|error| NativeFileError::Io(auris_io::IoError::from_fs(folder, error)))?;
        let staged = stage_native(&self.path, &self.document, self.limits)?;
        Ok(NativeSaveResult {
            owner: self.owner,
            generation: self.generation,
            revision: self.revision,
            path: self.path,
            original_path: self.original_path,
            staged,
            document: self.document,
            limits: self.limits,
            legacy: self.legacy,
            expected: self.expected,
            original_folder: self.original_folder,
        })
    }
}

impl NativeSaveResult {
    fn publish(self) -> NativeResult<()> {
        if let Some(candidate) = self.legacy {
            drop(self.staged);
            let backup = self.path.with_file_name(format!(
                "{}.legacy-backup",
                self.path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("project")
            ));
            auris_io::publish_native_upgrade_document(
                &candidate,
                &self.document,
                self.path.as_path(),
                self.path.as_path(),
                &backup,
                self.limits,
            )
        } else if self.expected.is_some() {
            let expected = self
                .expected
                .as_deref()
                .ok_or(NativeFileError::DestinationChanged(self.path.clone()))?;
            auris_io::publish_native_update(self.staged, expected, self.limits)
        } else {
            auris_io::publish_native_new_with(self.staged, || {
                let folder = self.path.parent().ok_or_else(|| {
                    NativeFileError::Validation("new project has no folder".into())
                })?;
                for entry in std::fs::read_dir(folder)
                    .map_err(|error| auris_io::IoError::from_fs(folder, error))?
                {
                    let entry = entry.map_err(|error| auris_io::IoError::from_fs(folder, error))?;
                    if entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("auris"))
                    {
                        return Err(NativeFileError::Validation(
                            "project folder already contains another document".into(),
                        ));
                    }
                }
                if self.path.parent() != self.original_folder.as_deref() {
                    copy_inside_assets(
                        &self.document,
                        self.original_folder.as_deref(),
                        self.path.parent(),
                    )
                    .map_err(|error| match error {
                        NativeFileSessionError::Io(io) => io,
                        other => NativeFileError::Io(auris_io::IoError::from_fs(
                            &self.path,
                            std::io::Error::other(other.to_string()),
                        )),
                    })?;
                }
                Ok(())
            })
        }
    }
}

fn copy_inside_assets(
    document: &NativeDocument,
    source_folder: Option<&Path>,
    destination_folder: Option<&Path>,
) -> Result<(), NativeFileSessionError> {
    let Some(destination_folder) = destination_folder else {
        return Err(NativeFileSessionError::AssetCopy(
            "destination has no project folder".into(),
        ));
    };
    let mut assets: Vec<(AssetPath, bool)> = Vec::new();
    for source in document.context.audio_sources.values() {
        if source.path.is_inside() {
            assets.push((source.path.clone(), false));
        }
    }
    for font in document.context.soundfonts.values() {
        if font.path.is_inside() {
            assets.push((font.path.clone(), false));
        }
    }
    for processor in &document.content.processors {
        match &processor.kind {
            auris_core::pipeline::content::ProcessorKind::Instrument(plugin)
            | auris_core::pipeline::content::ProcessorKind::Effect { plugin, .. } => {
                if let Some(path) = &plugin.file
                    && path.is_inside()
                {
                    assets.push((path.clone(), false));
                }
            }
            auris_core::pipeline::content::ProcessorKind::Singer(singer) => {
                if let Some(voice) = &singer.voice
                    && voice.path.is_inside()
                {
                    assets.push((voice.path.clone(), true));
                }
            }
            _ => {}
        }
    }
    for (asset, bundle) in assets {
        let source = asset.resolve(source_folder).ok_or_else(|| {
            NativeFileSessionError::AssetCopy("inside asset has no source folder".into())
        })?;
        let AssetPath::Inside(relative) = asset else {
            continue;
        };
        let destination = AssetPath::inside(&relative)
            .resolve(Some(destination_folder))
            .ok_or_else(|| NativeFileSessionError::AssetCopy("invalid inside asset path".into()))?;
        if bundle {
            // VOICEVOX connections are self-contained. File-backed model manifests can name
            // companion weights; copying only their entry file would silently break reopen.
            let voicevox = source
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(".voicevox.json"));
            if !voicevox {
                return Err(NativeFileSessionError::AssetCopy(
                    "inside voice model requires its complete model assets".into(),
                ));
            }
            let checked = auris_singer::validate_automatic_voice_entry(&source)
                .map_err(|error| NativeFileSessionError::AssetCopy(error.to_string()))?;
            auris_singer::VoiceModel::load(&checked, auris_singer::Acceleration::Cpu)
                .map_err(|error| NativeFileSessionError::AssetCopy(error.to_string()))?;
        }
        copy_file_noclobber(&source, &destination)?;
    }
    Ok(())
}

fn copy_file_noclobber(source: &Path, destination: &Path) -> Result<(), NativeFileSessionError> {
    if !source.is_file() {
        return Err(NativeFileSessionError::AssetCopy(format!(
            "missing {}",
            source.display()
        )));
    }
    if destination.is_file() {
        if files_equal(source, destination)? {
            return Ok(());
        }
        return Err(NativeFileSessionError::AssetCopy(format!(
            "destination changed {}",
            destination.display()
        )));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?;
    }
    let parent = destination.parent().ok_or_else(|| {
        NativeFileSessionError::AssetCopy("destination has no parent folder".into())
    })?;
    let input = std::fs::File::open(source)
        .map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?;
    match copy_reader_noclobber(input, parent, destination) {
        Ok(()) => Ok(()),
        Err(_error) if destination.is_file() && files_equal(source, destination)? => Ok(()),
        Err(error) => Err(error),
    }
}

fn copy_reader_noclobber<R: std::io::Read>(
    mut input: R,
    parent: &Path,
    destination: &Path,
) -> Result<(), NativeFileSessionError> {
    // NamedTempFile removes the staged file on every error path. Persisting it with
    // `persist_noclobber` keeps a concurrently-created destination intact.
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?;
    std::io::copy(&mut input, temporary.as_file_mut())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?;
    temporary
        .persist_noclobber(destination)
        .map(|_| ())
        .map_err(|error| NativeFileSessionError::AssetCopy(error.error.to_string()))
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, NativeFileSessionError> {
    let left_len = std::fs::metadata(left)
        .map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?
        .len();
    let right_len = std::fs::metadata(right)
        .map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?
        .len();
    if left_len != right_len {
        return Ok(false);
    }
    let mut left = std::io::BufReader::new(
        std::fs::File::open(left).map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?,
    );
    let mut right = std::io::BufReader::new(
        std::fs::File::open(right).map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?,
    );
    let mut left_buf = [0_u8; 64 * 1024];
    let mut right_buf = [0_u8; 64 * 1024];
    loop {
        let left_read = std::io::Read::read(&mut left, &mut left_buf)
            .map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?;
        let right_read = std::io::Read::read(&mut right, &mut right_buf)
            .map_err(|e| NativeFileSessionError::AssetCopy(e.to_string()))?;
        if left_read != right_read || left_buf[..left_read] != right_buf[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}
