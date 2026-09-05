//! Named document snapshots for comparisons and recovery across headless tool calls.
//!
//! Snapshots stay in the project's folder; asset references keep their original origin.
//! Audio files are not duplicated, so collecting assets remains a separate command.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use auris_core::Project;
use auris_io::{IoError, load_project, save_project};

use super::Session;
use crate::SessionError;

impl Session {
    fn checkpoint_path(&self, name: &str) -> Result<PathBuf, SessionError> {
        if name.is_empty()
            || name.len() > 80
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(SessionError::InvalidCheckpointName);
        }
        let folder = self.project_folder().ok_or(SessionError::NoPath)?;
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
        if load_project(&path)? != self.saved_project {
            return Err(SessionError::ExternalChanges(path));
        }
        if self.project != self.saved_project {
            self.preserve_document(&self.saved_project)?;
        }
        self.save_in_place()
    }

    /// Preserves a document before an explicit replacement or a headless edit.
    pub(super) fn preserve_document(&self, project: &Project) -> Result<PathBuf, SessionError> {
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
        self.checkpoint_document(&name, project)
    }
}

#[cfg(test)]
mod tests {
    use crate::session::fixtures::{Scratch, session};

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
