//! Named document snapshots for comparisons and recovery across headless tool calls.
//!
//! Snapshots stay in the project's folder; asset references keep their original origin.
//! Audio files are not duplicated, so collecting assets remains a separate command.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use auris_core::Project;
use auris_io::{
    IoError, StagedProject, load_project, save_session_project as save_project,
    stage_session_project as stage_project,
};

use super::Session;
use crate::SessionError;

/// Explicit document-history operation, shared by saved and live agents.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckpointOperation {
    /// Page at most 128 retained checkpoint names.
    List {
        /// Zero-based sorted name offset.
        #[serde(default)]
        offset: usize,
    },
    /// Preserve the current document under a new name; existing names are refused.
    Create {
        /// 1..80 bytes of letters, digits, hyphens or underscores.
        name: String,
    },
    /// Restore one snapshot as an undoable edit; requires the current document revision.
    Restore {
        /// Exact name returned by list or create.
        name: String,
    },
}

impl CheckpointOperation {
    /// Whether the operation replaces current document content.
    pub fn is_mutating(&self) -> bool {
        matches!(self, Self::Restore { .. })
    }
}

/// History work detached from its originating document thread.
pub struct CheckpointJob {
    owner: std::sync::Arc<auris_core::PluginRegistry>,
    revision: u64,
    content_revision: String,
    folder: PathBuf,
    project: Project,
    render_rate: f64,
    operation: CheckpointOperation,
}

/// A private staged checkpoint or prepared restoration awaiting owner validation.
pub struct CheckpointResult {
    owner: std::sync::Arc<auris_core::PluginRegistry>,
    revision: u64,
    content_revision: String,
    folder: PathBuf,
    outcome: CheckpointOutcome,
}

enum CheckpointOutcome {
    Listed(serde_json::Value),
    Created {
        name: String,
        staged: StagedProject,
    },
    Restored {
        name: String,
        project: Box<Project>,
        assets: super::assets::PreparedAssets,
    },
}

impl CheckpointJob {
    /// Read, serialize or prepare assets on a worker, without publishing or editing.
    pub fn run(
        self,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<CheckpointResult, String> {
        let check = || {
            if cancelled.load(Ordering::Relaxed) {
                Err("History operation cancelled".to_string())
            } else {
                Ok(())
            }
        };
        check()?;
        let outcome = match self.operation {
            CheckpointOperation::List { offset } => {
                let path = checkpoint_path_in_folder(&self.folder, "listing")
                    .map_err(|e| e.to_string())?;
                let mut names = Vec::new();
                for entry in std::fs::read_dir(path.parent().expect("history folder"))
                    .map_err(|e| e.to_string())?
                {
                    check()?;
                    let entry = entry.map_err(|e| e.to_string())?;
                    if let Some(name) = entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_prefix("checkpoint-"))
                        .and_then(|name| name.strip_suffix(".json"))
                    {
                        if names.len() >= 10000 {
                            return Err("History exceeds the 10000-checkpoint listing limit".into());
                        }
                        names.push(name.to_owned());
                    }
                }
                names.sort();
                if offset > names.len() {
                    return Err("offset exceeds checkpoint count".into());
                }
                let next = (offset + 128).min(names.len());
                CheckpointOutcome::Listed(
                    serde_json::json!({"names":&names[offset..next],"total":names.len(),"next_offset":(next<names.len()).then_some(next)}),
                )
            }
            CheckpointOperation::Create { name } => {
                let path =
                    checkpoint_path_in_folder(&self.folder, &name).map_err(|e| e.to_string())?;
                if path.exists() {
                    return Err("Checkpoint name already exists".into());
                }
                let staged =
                    stage_project(&path, &mut self.project.clone()).map_err(|e| e.to_string())?;
                CheckpointOutcome::Created { name, staged }
            }
            CheckpointOperation::Restore { name } => {
                let path =
                    checkpoint_path_in_folder(&self.folder, &name).map_err(|e| e.to_string())?;
                let resolved = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
                let directory = std::fs::canonicalize(path.parent().expect("history folder"))
                    .map_err(|e| e.to_string())?;
                if !resolved.starts_with(directory) {
                    return Err("Checkpoint escapes its history folder".into());
                }
                let project = load_project(&path).map_err(|e| e.to_string())?;
                let assets = super::assets::prepare_project_assets(
                    &project,
                    Some(&self.folder),
                    self.render_rate,
                    cancelled,
                )
                .ok_or("History operation cancelled")?;
                CheckpointOutcome::Restored {
                    name,
                    project: Box::new(project),
                    assets,
                }
            }
        };
        check()?;
        Ok(CheckpointResult {
            owner: self.owner,
            revision: self.revision,
            content_revision: self.content_revision,
            folder: self.folder,
            outcome,
        })
    }
}

impl Session {
    /// Capture history work and reject a restore aimed at an obsolete document revision.
    pub fn begin_agent_checkpoints(
        &self,
        operation: CheckpointOperation,
        expected_revision: Option<String>,
    ) -> Result<CheckpointJob, String> {
        if self.transaction.is_some() {
            return Err("An edit is already in progress".into());
        }
        let content_revision = self.agent_document_revision()?;
        if operation.is_mutating() && expected_revision.as_deref() != Some(&content_revision) {
            return Err("Document changed; inspect_project before restoring a checkpoint".into());
        }
        Ok(CheckpointJob {
            owner: std::sync::Arc::clone(&self.registry),
            revision: self.revision,
            content_revision,
            folder: self
                .project_folder()
                .ok_or("Missing project folder")?
                .to_owned(),
            project: self.project.clone(),
            render_rate: self.engine.sample_rate(),
            operation,
        })
    }

    /// Accept a current history result; restoration retains the current file and one Undo step.
    pub fn continue_agent_checkpoints(
        &mut self,
        result: CheckpointResult,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<String, String> {
        if cancelled.load(Ordering::Relaxed)
            || self.revision != result.revision
            || !std::sync::Arc::ptr_eq(&self.registry, &result.owner)
            || self.project_folder() != Some(result.folder.as_path())
        {
            return Err("Document changed or history operation cancelled; nothing applied".into());
        }
        let mut response = match result.outcome {
            CheckpointOutcome::Listed(response) => response,
            CheckpointOutcome::Created { name, staged } => {
                staged.publish_noclobber().map_err(|e| e.to_string())?;
                serde_json::json!({"created":name})
            }
            CheckpointOutcome::Restored {
                name,
                mut project,
                assets,
            } => {
                if self.engine.sample_rate() != assets.render_rate {
                    return Err("Audio device rate changed during checkpoint preparation".into());
                }
                // The open document keeps its file's build provenance until the next save.
                project.saved_by.clone_from(&self.project.saved_by);
                if self.project == *project {
                    serde_json::json!({"restored":name,"changed":false})
                } else {
                    self.record(crate::Edit::ExternalChanges);
                    self.hosted.clear();
                    self.vst3.clear();
                    self.adopt_project(*project);
                    let missing = self.install_prepared_assets(assets);
                    self.invalidate_graph();
                    self.publish_loop();
                    serde_json::json!({"restored":name,"changed":true,"missing_assets":missing})
                }
            }
        };
        response["revision"] = self.agent_document_revision()?.into();
        response["captured_revision"] = result.content_revision.into();
        Ok(response.to_string())
    }
    fn checkpoint_path(&self, name: &str) -> Result<PathBuf, SessionError> {
        let folder = self.project_folder().ok_or(SessionError::NoPath)?;
        checkpoint_path_in_folder(folder, name)
    }

    fn checkpoint_document(&self, name: &str, project: &Project) -> Result<PathBuf, SessionError> {
        let path = self.checkpoint_path(name)?;
        if path.exists() {
            return Err(SessionError::WouldReplace(path));
        }
        save_project(&path, &mut project.clone())?;
        Ok(path)
    }

    /// Saves the current document under a new name for a later comparison or restoration.
    ///
    /// Names contain letters, digits, hyphens or underscores, up to 80 UTF-8 bytes. Existing
    /// snapshots are never replaced. Audio and SoundFont files remain in their original places.
    pub fn create_checkpoint(&self, name: &str) -> Result<PathBuf, SessionError> {
        self.checkpoint_document(name, &self.project)
    }

    /// Lists checkpoint names in alphabetical order, without opening their documents.
    pub fn checkpoints(&self) -> Result<Vec<String>, SessionError> {
        let directory = self
            .checkpoint_path("listing")?
            .parent()
            .unwrap()
            .to_path_buf();
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&directory).map_err(|e| IoError::from_fs(&directory, e))? {
            let entry = entry.map_err(|e| IoError::from_fs(&directory, e))?;
            if let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("checkpoint-"))
                .and_then(|name| name.strip_suffix(".json"))
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Restores a checkpoint into the current document as an undoable edit; does not save.
    pub fn restore_checkpoint(&mut self, name: &str) -> Result<Vec<PathBuf>, SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        let path = self.checkpoint_path(name)?;
        let resolved = std::fs::canonicalize(&path).map_err(|e| IoError::from_fs(&path, e))?;
        let directory = std::fs::canonicalize(path.parent().unwrap())
            .map_err(|e| IoError::from_fs(&path, e))?;
        if !resolved.starts_with(directory) {
            return Err(SessionError::InvalidCheckpointName);
        }
        let project = load_project(&path)?;
        self.record(crate::Edit::ExternalChanges);
        Ok(self.replace_external_project(project))
    }

    /// Saves an edited document after preserving the disk version in an automatic checkpoint.
    ///
    /// Headless tools use this because their in-memory undo history ends with the call. A
    /// stale session refuses before making a snapshot or changing the project file.
    pub fn save_with_checkpoint(&mut self) -> Result<(), SessionError> {
        let path = self.path.clone().ok_or(SessionError::NoPath)?;
        if !super::files::same_persisted_project(&load_project(&path)?, &self.saved_project) {
            return Err(SessionError::ExternalChanges(path));
        }
        if self.project != self.saved_project {
            self.preserve_document(&self.saved_project)?;
        }
        self.save_in_place()
    }

    /// Preserves a document before an explicit replacement or a headless edit.
    pub(super) fn preserve_document(&self, project: &Project) -> Result<PathBuf, SessionError> {
        let folder = self.project_folder().ok_or(SessionError::NoPath)?;
        preserve_document_in_folder(folder, project)
    }
}

fn checkpoint_path_in_folder(folder: &Path, name: &str) -> Result<PathBuf, SessionError> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(SessionError::InvalidCheckpointName);
    }
    let directory = folder.join(".auris-history");
    std::fs::create_dir_all(&directory).map_err(|e| IoError::from_fs(&directory, e))?;
    let canonical =
        std::fs::canonicalize(&directory).map_err(|e| IoError::from_fs(&directory, e))?;
    let parent = std::fs::canonicalize(folder).map_err(|e| IoError::from_fs(folder, e))?;
    if !canonical.starts_with(&parent) {
        return Err(SessionError::InvalidCheckpointName);
    }
    Ok(directory.join(format!("checkpoint-{name}.json")))
}

/// Preserves a document in a known project folder without borrowing the live session.
pub(super) fn preserve_document_in_folder(
    folder: &Path,
    project: &Project,
) -> Result<PathBuf, SessionError> {
    let (path, staged) = stage_preserved_document_in_folder(folder, project)?;
    staged.publish()?;
    Ok(path)
}

/// Stages an automatic checkpoint without making it visible until the caller commits.
pub(super) fn stage_preserved_document_in_folder(
    folder: &Path,
    project: &Project,
) -> Result<(PathBuf, StagedProject), SessionError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!(
        "auto-{timestamp}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let path = checkpoint_path_in_folder(folder, &name)?;
    let staged = stage_project(&path, &mut project.clone())?;
    Ok((path, staged))
}

#[cfg(test)]
mod tests {
    use crate::session::fixtures::{Scratch, session};

    #[test]
    fn agent_history_restores_one_undo_and_rejects_concurrent_edits_and_name_races() {
        use super::*;
        let mut live = session();
        live.add_bus_track("Original");
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let saved_path = live.path().map(Path::to_owned);
        let snapshot = live.project().clone();
        let capture = || CheckpointOperation::Create {
            name: "TakeA".into(),
        };
        let first = live
            .begin_agent_checkpoints(capture(), None)
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let raced = live
            .begin_agent_checkpoints(capture(), None)
            .unwrap()
            .run(&cancelled)
            .unwrap();
        live.continue_agent_checkpoints(first, &cancelled).unwrap();
        assert!(live.continue_agent_checkpoints(raced, &cancelled).is_err());
        assert_eq!(live.project(), &snapshot);
        live.add_bus_track("Other take");
        let other = live.project().clone();
        let restore = || CheckpointOperation::Restore {
            name: "TakeA".into(),
        };
        assert!(live.begin_agent_checkpoints(restore(), None).is_err());
        let result = live
            .begin_agent_checkpoints(restore(), Some(live.agent_document_revision().unwrap()))
            .unwrap()
            .run(&cancelled)
            .unwrap();
        live.continue_agent_checkpoints(result, &cancelled).unwrap();
        assert_eq!(live.project(), &snapshot);
        assert_eq!(live.path(), saved_path.as_deref());
        live.undo();
        assert_eq!(live.project(), &other);
        let result = live
            .begin_agent_checkpoints(restore(), Some(live.agent_document_revision().unwrap()))
            .unwrap()
            .run(&cancelled)
            .unwrap();
        live.add_bus_track("User edit while waiting");
        let concurrent = live.project().clone();
        assert!(live.continue_agent_checkpoints(result, &cancelled).is_err());
        assert_eq!(live.project(), &concurrent);
        let listed = live
            .begin_agent_checkpoints(CheckpointOperation::List { offset: 0 }, None)
            .unwrap()
            .run(&cancelled)
            .unwrap();
        let listed: serde_json::Value =
            serde_json::from_str(&live.continue_agent_checkpoints(listed, &cancelled).unwrap())
                .unwrap();
        assert_eq!(listed["names"], serde_json::json!(["TakeA"]));
    }

    #[test]
    fn snapshots_survive_fresh_sessions_and_restore_without_losing_the_other_take() {
        let scratch = Scratch::new("checkpoints");
        let path = scratch.join("Song.auris");
        let mut first = session();
        first.add_default_instrument_track("Original").unwrap();
        first.save(&path).unwrap();
        first.create_checkpoint("案A").unwrap();
        assert!(first.create_checkpoint("案A").is_err());
        first.add_default_instrument_track("Second take").unwrap();
        first.save_with_checkpoint().unwrap();
        let mut second = session();
        second.open(&path).unwrap();
        assert_eq!(second.checkpoints().unwrap().len(), 2);
        second.restore_checkpoint("案A").unwrap();
        second.save_with_checkpoint().unwrap();
        assert_eq!(second.project().tracks.len(), 1);
        assert_eq!(second.checkpoints().unwrap().len(), 3);
        assert_eq!(second.undo(), Some(crate::Edit::ExternalChanges));
        assert_eq!(second.project().tracks.len(), 2);
        for name in ["../escape", "", "a/b", "a\\b", "."] {
            assert!(second.create_checkpoint(name).is_err());
        }
    }
}
