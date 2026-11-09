//! The project workspace: a folder beside the document that agents read and write freely.
//!
//! An agent naming a local file has to be asked about it, because a path can be anything on the
//! machine. A path *relative to the workspace* cannot: it is checked here to stay inside
//! `<project folder>/Workspace`, so the permission policy can let it through without a prompt.
//! The folder is not an asset store — nothing in the document points into it, and a sample loaded
//! from it is copied into `Audio/` as any import is — but it belongs to the project folder, so
//! Save As, autosave recovery and a workshop candidate's checkpoints all carry it along.

use super::Session;
use crate::error::SessionError;
use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Most entries one workspace listing returns.
const LIST_LIMIT: usize = 256;
/// Deepest folder nesting a listing descends into.
const LIST_DEPTH: usize = 8;

/// Parses a workspace-relative path, without touching the disk.
///
/// `/` and `\` both separate components, so a path written on either platform means the same
/// file. Absolute paths, drive prefixes, `.` and `..` components, empty components and colons
/// (a Windows drive or alternate data stream) are refused: the result can only name something
/// below the workspace. `None` is not an error by itself — the caller may take the text as an
/// absolute local path instead.
pub fn workspace_relative(value: &str) -> Option<PathBuf> {
    if value.is_empty()
        || value.len() > 1024
        || value.starts_with(['/', '\\'])
        || Path::new(value).is_absolute()
    {
        return None;
    }
    let mut relative = PathBuf::new();
    for segment in value.split(['/', '\\']) {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains(':')
            || segment.chars().any(char::is_control)
        {
            return None;
        }
        relative.push(segment);
    }
    // A last check against whatever the platform itself would read into the components.
    relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then_some(relative)
}

/// Rebuilds `path` from its deepest existing ancestor with links resolved.
fn resolve_existing_prefix(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing
                    .file_name()
                    .ok_or("Workspace path has no existing ancestor")?
                    .to_owned();
                rest.push(name);
                existing = existing
                    .parent()
                    .ok_or("Workspace path has no existing ancestor")?
                    .to_path_buf();
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    let mut resolved = existing.canonicalize().map_err(|e| e.to_string())?;
    for name in rest.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// Copies every regular file under `from` into the same place under `to`.
///
/// Links are skipped rather than followed, so a workspace cannot smuggle a file from elsewhere
/// into the copy. A file already at its destination is left alone. Returns `false` when
/// cancelled part way, and the paths that could not be copied.
pub(crate) fn copy_tree(
    from: &Path,
    to: &Path,
    cancelled: &AtomicBool,
) -> Result<(bool, Vec<PathBuf>), SessionError> {
    let mut failed = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        if cancelled.load(Ordering::Relaxed) {
            return Ok((false, failed));
        }
        let Ok(entries) = fs::read_dir(from.join(&relative)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = relative.join(entry.file_name());
            if kind.is_dir() {
                pending.push(name);
            } else if kind.is_file() {
                let destination = to.join(&name);
                if destination.exists() {
                    continue;
                }
                let copied = destination
                    .parent()
                    .map_or(Ok(()), fs::create_dir_all)
                    .and_then(|()| fs::copy(entry.path(), &destination));
                if let Err(error) = copied {
                    log::warn!("could not copy workspace file {}: {error}", name.display());
                    failed.push(entry.path());
                }
            }
        }
    }
    Ok((!cancelled.load(Ordering::Relaxed), failed))
}

/// Copies one project folder's workspace into another's, unless they are the same folder.
pub(crate) fn carry_workspace(
    from_folder: &Path,
    to_folder: &Path,
    cancelled: &AtomicBool,
) -> Result<(bool, Vec<PathBuf>), SessionError> {
    let from = from_folder.join(auris_io::WORKSPACE_DIR);
    let to = to_folder.join(auris_io::WORKSPACE_DIR);
    if !from.is_dir()
        || from.canonicalize().ok() == to.canonicalize().ok()
        || from_folder == to_folder
    {
        return Ok((true, Vec::new()));
    }
    copy_tree(&from, &to, cancelled)
}

impl Session {
    /// The open project's workspace folder, which may not exist yet.
    ///
    /// An unsaved document's workspace lives in its private working folder and moves with the
    /// document when it is first saved.
    pub fn workspace_folder(&self) -> PathBuf {
        self.project_folder()
            .expect("every session has a working project folder")
            .join(auris_io::WORKSPACE_DIR)
    }

    /// Resolves a workspace-relative path to an absolute one, creating the workspace.
    ///
    /// Refuses a path that leaves the workspace lexically or through a link. The file itself need
    /// not exist; its folders are not created here.
    pub fn resolve_workspace_path(&self, value: &str) -> Result<PathBuf, String> {
        let relative = workspace_relative(value).ok_or(
            "Workspace paths are relative, use / between folders and never contain '..', '.', ':' or empty parts",
        )?;
        let root = self.workspace_folder();
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
        let path = root.join(&relative);
        if !resolve_existing_prefix(&path)?.starts_with(&canonical_root) {
            return Err("Workspace path leaves the project workspace through a link".into());
        }
        Ok(path)
    }

    /// The workspace-relative form of an absolute path inside the workspace, `/`-separated.
    pub fn workspace_path_of(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(self.workspace_folder()).ok()?;
        let parts: Vec<_> = relative
            .components()
            .map(|component| match component {
                Component::Normal(part) => part.to_str(),
                _ => None,
            })
            .collect::<Option<_>>()?;
        (!parts.is_empty()).then(|| parts.join("/"))
    }

    /// Lists files below a workspace folder (`folder` relative, or the root) for an agent.
    pub fn agent_list_workspace(&self, folder: Option<&str>) -> Result<String, String> {
        let root = self.workspace_folder();
        let start = match folder {
            Some(folder) => self.resolve_workspace_path(folder)?,
            None => root.clone(),
        };
        let mut files = Vec::new();
        let mut truncated = false;
        let mut pending = vec![(start.clone(), 0usize)];
        while let Some((directory, depth)) = pending.pop() {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(fs::DirEntry::file_name);
            for entry in entries {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    if depth + 1 < LIST_DEPTH {
                        pending.push((path, depth + 1));
                    }
                } else if kind.is_file() {
                    if files.len() == LIST_LIMIT {
                        truncated = true;
                        break;
                    }
                    let Some(relative) = self.workspace_path_of(&path) else {
                        continue;
                    };
                    let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    files.push(serde_json::json!({"path":relative,"bytes":bytes}));
                }
            }
        }
        files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        Ok(serde_json::json!({
            "workspace": root,
            "folder": folder.unwrap_or(""),
            "files": files,
            "truncated": truncated,
            "note": "Pass these relative paths to render, export_midi, verify_render, load_sample, import_audio and import_midi; they need no permission prompt. The folder travels with the project; nothing in it is part of the song until imported."
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_cannot_leave_the_workspace() {
        assert_eq!(
            workspace_relative("renders/mix.wav"),
            Some(Path::new("renders").join("mix.wav"))
        );
        assert_eq!(
            workspace_relative("renders\\mix.wav"),
            Some(Path::new("renders").join("mix.wav"))
        );
        for refused in [
            "",
            "/etc/passwd",
            "\\\\server\\share",
            "../outside.wav",
            "a/../../b.wav",
            "./mix.wav",
            "a//b.wav",
            "C:\\mix.wav",
            "C:mix.wav",
            "mix.wav:stream",
            "trailing/",
        ] {
            assert_eq!(workspace_relative(refused), None, "{refused}");
        }
    }

    #[test]
    fn resolution_creates_the_workspace_and_refuses_links_out() {
        let session = Session::new(crate::SessionOptions::headless()).unwrap();
        let path = session.resolve_workspace_path("takes/one.wav").unwrap();
        assert!(session.workspace_folder().is_dir());
        assert_eq!(
            path,
            session.workspace_folder().join("takes").join("one.wav")
        );
        assert_eq!(
            session.workspace_path_of(&path).as_deref(),
            Some("takes/one.wav")
        );
        assert!(session.resolve_workspace_path("../escape.wav").is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), session.workspace_folder().join("out"))
                .unwrap();
            assert!(session.resolve_workspace_path("out/file.wav").is_err());
        }
    }

    #[test]
    fn listing_reports_relative_paths_and_sizes() {
        let session = Session::new(crate::SessionOptions::headless()).unwrap();
        let root = session.workspace_folder();
        fs::create_dir_all(root.join("stems")).unwrap();
        fs::write(root.join("stems").join("bass.wav"), [0u8; 12]).unwrap();
        fs::write(root.join("idea.mid"), [0u8; 3]).unwrap();
        let listing: serde_json::Value =
            serde_json::from_str(&session.agent_list_workspace(None).unwrap()).unwrap();
        let files: Vec<_> = listing["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| {
                (
                    file["path"].as_str().unwrap(),
                    file["bytes"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(files, [("idea.mid", 3), ("stems/bass.wav", 12)]);
    }

    #[test]
    fn save_as_carries_the_workspace_to_the_new_folder() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let path = session.resolve_workspace_path("renders/take.wav").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"take").unwrap();
        let folder = tempfile::tempdir().unwrap();
        session.save_as(&folder.path().join("Song.auris")).unwrap();
        let carried = folder
            .path()
            .join("Song")
            .join(auris_io::WORKSPACE_DIR)
            .join("renders")
            .join("take.wav");
        assert_eq!(fs::read(&carried).unwrap(), b"take");
        assert_eq!(
            session.workspace_folder(),
            carried.parent().unwrap().parent().unwrap()
        );
        // A second Save As elsewhere carries it again, from the saved folder this time.
        let other = tempfile::tempdir().unwrap();
        session.save_as(&other.path().join("Copy.auris")).unwrap();
        assert!(
            other
                .path()
                .join("Copy")
                .join(auris_io::WORKSPACE_DIR)
                .join("renders")
                .join("take.wav")
                .is_file()
        );
    }
}
