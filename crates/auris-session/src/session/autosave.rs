//! Keeping a recovery snapshot while somebody is working on a document.
//!
//! # What this does and does not protect
//!
//! It writes a recovery copy in the session's private working folder. The file the user opened or
//! explicitly saved is never touched by autosave, so closing without saving keeps its ordinary
//! meaning and another process editing that file cannot be overwritten by a background tick.
//! An autosave-enabled session puts that working folder under a per-user registry. Normal session
//! destruction removes it; an abrupt process exit or panic unwind leaves a completed snapshot for
//! the next process to discover. An OS file lock distinguishes that abandoned folder from a
//! workspace another process still owns; the operating system releases the lock automatically
//! when its process exits.
//!
//! # What it will not do
//!
//! **It never fires mid-gesture.** A drag is one undo step and, until it ends, a document caught
//! halfway through a change nobody has finished making.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::hash::{DefaultHasher, Hasher};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use auris_core::{AssetPath, Project};
use serde::{Deserialize, Serialize};

use super::Session;
use super::assets::{PreparedAssets, prepare_project_assets};
use crate::error::SessionError;

const AUTOSAVE_FILE: &str = "Autosave.auris";
const METADATA_FILE: &str = "Recovery.json";
const LEASE_FILE: &str = "Session.lock";
const RECOVERY_FOLDER: &str = "auris-studio-recovery";
const WORKSPACE_PREFIX: &str = "session-";
const DISCARD_QUARANTINE_PREFIX: &str = ".discarded-";
const TEMP_WORKSPACE_PREFIX: &str = "auris-studio-";
const UNREGISTERED_RECOVERY_PREFIX: &str = "auris-studio-recovery-fallback-";
const METADATA_VERSION: u32 = 1;
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const MAX_METADATA_PROJECT_NAME_BYTES: usize = 512;
const MAX_METADATA_SOURCE_PATH_BYTES: usize = 4 * 1024;

/// Environment variable that overrides the recovery registry directory.
///
/// Intended for portable installations, tests and machines whose ordinary user-state location
/// is not writable. Unlike [`crate::CONFIG_DIR_VAR`], this directory can contain recorded audio
/// and should not be placed in a version-controlled dotfiles checkout.
pub const RECOVERY_DIR_VAR: &str = "AURIS_RECOVERY_DIR";

/// Preferred directory containing crash-recovery workspaces.
///
/// This is user state rather than configuration: a workspace may hold recorded or generated
/// audio as well as the project snapshot. The platform's persistent local-state location is used
/// where one is available. Session creation falls back to a known system-temporary registry, and
/// finally ordinary anonymous scratch storage, when this directory cannot be written.
pub fn recovery_dir() -> PathBuf {
    if let Some(override_dir) = std::env::var_os(RECOVERY_DIR_VAR).filter(|value| !value.is_empty())
    {
        return PathBuf::from(override_dir);
    }

    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .map(|home| home.join("AppData").join("Local"))
            })
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|home| home.join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .map(|home| home.join(".local").join("state"))
            })
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join(RECOVERY_FOLDER)
}

/// One autosave left by a session that did not shut down normally.
///
/// Values are obtained from [`Session::recovery_snapshots`]. Their filesystem identity stays
/// private so recovery and deletion can validate the exact registry entry before touching it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoverySnapshot {
    registry: PathBuf,
    workspace: PathBuf,
    document: PathBuf,
    project_name: Option<String>,
    source_document: Option<PathBuf>,
    modified: SystemTime,
}

impl RecoverySnapshot {
    /// Project title recorded beside the snapshot, if its metadata was intact.
    pub fn project_name(&self) -> Option<&str> {
        self.project_name.as_deref()
    }

    /// Permanent project the recovered edits were based on, if one had been chosen.
    ///
    /// Recovery itself never writes this path. It always restores an unsaved document so the
    /// caller must take an explicit Save or Save As path afterwards.
    pub fn source_document(&self) -> Option<&Path> {
        self.source_document.as_deref()
    }

    /// When the project snapshot was last written.
    pub fn modified(&self) -> SystemTime {
        self.modified
    }

    /// Path of the read-only recovery document.
    ///
    /// Frontends may show or reveal this path. They should recover it through
    /// [`Session::recover_autosave`] rather than opening it as a permanent project.
    pub fn document(&self) -> &Path {
        &self.document
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct RecoveryMetadata {
    version: u32,
    project_name: Option<String>,
    source_document: Option<PathBuf>,
}

/// The session's scratch directory and, for a registered workspace, its process lease.
///
/// Field order is intentional: the open file releases its lock before [`tempfile::TempDir`]
/// removes the directory during an ordinary shutdown.
pub(super) struct SessionWorkspace {
    _lease: Option<File>,
    directory: Option<tempfile::TempDir>,
    registered: bool,
}

impl SessionWorkspace {
    fn anonymous(directory: tempfile::TempDir) -> Self {
        Self {
            _lease: None,
            directory: Some(directory),
            registered: false,
        }
    }

    fn registered(directory: tempfile::TempDir, lease: File) -> Self {
        Self {
            _lease: Some(lease),
            directory: Some(directory),
            registered: true,
        }
    }

    pub(super) fn path(&self) -> &Path {
        self.directory
            .as_ref()
            .expect("a live session workspace has a directory")
            .path()
    }

    fn keep_for_recovery(&mut self) {
        // Close the lease first: the next process must be able to acquire it as soon as the
        // unwinding thread has finished preserving the directory.
        drop(self._lease.take());
        if let Some(directory) = self.directory.take() {
            let _ = directory.keep();
        }
    }
}

impl Drop for SessionWorkspace {
    fn drop(&mut self) {
        let has_snapshot = self
            .directory
            .as_ref()
            .is_some_and(|directory| directory.path().join(AUTOSAVE_FILE).is_file());
        if self.registered && std::thread::panicking() && has_snapshot {
            self.keep_for_recovery();
        }
    }
}

fn io_error(path: &Path, source: std::io::Error) -> SessionError {
    auris_io::IoError::from_fs(path, source).into()
}

fn fallback_recovery_dir() -> PathBuf {
    std::env::temp_dir().join(RECOVERY_FOLDER)
}

fn create_temporary_workspace() -> Result<SessionWorkspace, SessionError> {
    let directory = tempfile::Builder::new()
        .prefix(TEMP_WORKSPACE_PREFIX)
        .tempdir()
        .map_err(|error| io_error(&std::env::temp_dir(), error))?;
    Ok(SessionWorkspace::anonymous(directory))
}

fn create_unregistered_recovery_workspace() -> Result<SessionWorkspace, SessionError> {
    let directory = tempfile::Builder::new()
        .prefix(UNREGISTERED_RECOVERY_PREFIX)
        .tempdir()
        .map_err(|error| io_error(&std::env::temp_dir(), error))?;
    Ok(SessionWorkspace::anonymous(directory))
}

pub(super) fn create_session_workspace(autosave: bool) -> Result<SessionWorkspace, SessionError> {
    if !autosave {
        return create_temporary_workspace();
    }

    let primary = recovery_dir();
    create_registered_workspace(&primary, &fallback_recovery_dir())
}

fn create_registered_workspace(
    primary: &Path,
    fallback: &Path,
) -> Result<SessionWorkspace, SessionError> {
    match create_recovery_workspace_in(primary) {
        Ok(workspace) => return Ok(workspace),
        Err(error) => log::warn!(
            "could not create recovery workspace in {}: {error}",
            primary.display()
        ),
    }

    if fallback != primary {
        match create_recovery_workspace_in(fallback) {
            Ok(workspace) => return Ok(workspace),
            Err(error) => log::warn!(
                "could not create fallback recovery workspace in {}: {error}",
                fallback.display()
            ),
        }
    }

    // Recovery is best effort. An unwritable state directory must not turn a recoverability
    // feature into an application-startup failure; the session still gets ordinary scratch
    // storage and explicit Save remains fully functional.
    create_unregistered_recovery_workspace()
}

fn create_recovery_workspace_in(root: &Path) -> Result<SessionWorkspace, SessionError> {
    fs::create_dir_all(root).map_err(|error| io_error(root, error))?;
    let directory = tempfile::Builder::new()
        .prefix(WORKSPACE_PREFIX)
        .tempdir_in(root)
        .map_err(|error| io_error(root, error))?;
    let lease = try_workspace_lease(directory.path())?
        .ok_or_else(|| SessionError::RecoveryUnavailable(directory.path().to_path_buf()))?;
    Ok(SessionWorkspace::registered(directory, lease))
}

fn try_workspace_lease(workspace: &Path) -> Result<Option<File>, SessionError> {
    let path = workspace.join(LEASE_FILE);
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (!metadata.is_file() || metadata.file_type().is_symlink() || is_reparse_point(&metadata))
    {
        return Err(SessionError::RecoveryUnavailable(path));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_error(&path, error))?;
    match file.try_lock() {
        Ok(()) => {
            let workspace =
                fs::canonicalize(workspace).map_err(|error| io_error(workspace, error))?;
            let lease = fs::canonicalize(&path).map_err(|error| io_error(&path, error))?;
            if lease != workspace.join(LEASE_FILE) {
                return Err(SessionError::RecoveryUnavailable(path));
            }
            Ok(Some(file))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(io_error(&path, error)),
    }
}

fn displayable_metadata(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn source_document(path: Option<&Path>) -> Option<PathBuf> {
    path.map(|path| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
        .and_then(|path| {
            let display = path.to_string_lossy();
            displayable_metadata(&display, MAX_METADATA_SOURCE_PATH_BYTES)
                .then(|| PathBuf::from(display.into_owned()))
        })
}

fn metadata_project_name(name: &str) -> Option<String> {
    displayable_metadata(name, MAX_METADATA_PROJECT_NAME_BYTES).then(|| name.to_owned())
}

fn recovery_metadata_bytes(
    project: &Project,
    source_document: Option<PathBuf>,
) -> Result<Vec<u8>, SessionError> {
    let metadata = RecoveryMetadata {
        version: METADATA_VERSION,
        project_name: metadata_project_name(&project.name),
        source_document,
    };
    serde_json::to_vec_pretty(&metadata)
        .map_err(auris_io::IoError::from)
        .map_err(SessionError::from)
}

fn read_metadata(path: &Path) -> Option<RecoveryMetadata> {
    let file = fs::symlink_metadata(path).ok()?;
    if !file.is_file()
        || file.file_type().is_symlink()
        || is_reparse_point(&file)
        || file.len() > MAX_METADATA_BYTES
    {
        return None;
    }
    // The metadata can change between inspection and opening. Bound the read itself as well as
    // checking the initial length so a replaced file cannot make a startup scan allocate without
    // limit.
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return None;
    }
    let mut metadata: RecoveryMetadata = serde_json::from_slice(&bytes).ok()?;
    if metadata.version != METADATA_VERSION {
        return None;
    }
    if metadata
        .project_name
        .as_ref()
        .is_some_and(|name| !displayable_metadata(name, MAX_METADATA_PROJECT_NAME_BYTES))
    {
        metadata.project_name = None;
    }
    if metadata.source_document.as_ref().is_some_and(|path| {
        !path.is_absolute()
            || !displayable_metadata(&path.to_string_lossy(), MAX_METADATA_SOURCE_PATH_BYTES)
    }) {
        metadata.source_document = None;
    }
    Some(metadata)
}

fn write_snapshot(
    document: &Path,
    project: &Project,
    source_document: Option<PathBuf>,
) -> Result<(), SessionError> {
    let metadata_path = document
        .parent()
        .ok_or_else(|| SessionError::RecoveryUnavailable(document.to_path_buf()))?
        .join(METADATA_FILE);
    let metadata = auris_io::stage_file_bytes(
        &metadata_path,
        &recovery_metadata_bytes(project, source_document)?,
    )?;
    let mut recovery = project.clone();
    let staged = auris_io::stage_project(document, &mut recovery)?;
    staged.publish()?;
    metadata.publish().map_err(SessionError::from)
}

fn make_recovery_paths_external(project: &mut Project, folder: &Path) {
    for source in project.audio_sources.values_mut() {
        if source.path.is_inside()
            && let Some(resolved) = source.path.resolve(Some(folder))
        {
            source.path = AssetPath::external(resolved);
        }
    }
    for font in project.soundfonts.values_mut() {
        if font.path.is_inside()
            && let Some(resolved) = font.path.resolve(Some(folder))
        {
            font.path = AssetPath::external(resolved);
        }
    }
    for track in &mut project.tracks {
        if let Some(voice) = track
            .kind
            .as_singer_mut()
            .and_then(|singer| singer.voice.as_mut())
            && voice.path.is_inside()
            && let Some(resolved) = voice.path.resolve(Some(folder))
        {
            voice.path = AssetPath::external(resolved);
        }
    }
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = metadata;
        false
    }
}

fn registry_workspace(root: &Path, workspace: &Path) -> bool {
    if workspace.parent() != Some(root) {
        return false;
    }
    let Some(name) = workspace.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !name.starts_with(WORKSPACE_PREFIX) {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(workspace) else {
        return false;
    };
    metadata.is_dir() && !metadata.file_type().is_symlink() && !is_reparse_point(&metadata)
}

fn registry_quarantine(root: &Path, quarantine: &Path) -> bool {
    if quarantine.parent() != Some(root) {
        return false;
    }
    let Some(name) = quarantine.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !name.starts_with(DISCARD_QUARANTINE_PREFIX) {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(quarantine) else {
        return false;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return false;
    }
    let (Ok(root), Ok(quarantine)) = (fs::canonicalize(root), fs::canonicalize(quarantine)) else {
        return false;
    };
    quarantine == root.join(name)
}

fn cleanup_recovery_quarantines_in(
    root: &Path,
    mut cleanup: impl FnMut(&Path) -> std::io::Result<()>,
) -> Result<(), SessionError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(root, error)),
    };
    let mut first_error = None;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                let error = io_error(root, error);
                if first_error.is_none() {
                    first_error = Some(error);
                } else {
                    log::warn!(
                        "could not inspect another entry in recovery registry {}: {error}",
                        root.display()
                    );
                }
                continue;
            }
        };
        let quarantine = entry.path();
        if !registry_quarantine(root, &quarantine) {
            continue;
        }
        if let Err(error) = cleanup(&quarantine) {
            if error.kind() == std::io::ErrorKind::NotFound {
                continue;
            }
            let error = io_error(&quarantine, error);
            if first_error.is_none() {
                first_error = Some(error);
            } else {
                log::warn!(
                    "could not remove another recovery quarantine {}: {error}",
                    quarantine.display()
                );
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn recovery_snapshots_in(root: &Path) -> Result<Vec<RecoverySnapshot>, SessionError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(root, error)),
    };
    let mut snapshots = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!(
                    "could not inspect one entry in recovery registry {}: {error}",
                    root.display()
                );
                continue;
            }
        };
        let workspace = entry.path();
        if !registry_workspace(root, &workspace) {
            continue;
        }
        let document = workspace.join(AUTOSAVE_FILE);
        let Ok(file) = fs::symlink_metadata(&document) else {
            continue;
        };
        if !file.is_file() || file.file_type().is_symlink() || is_reparse_point(&file) {
            continue;
        }
        let lease = match try_workspace_lease(&workspace) {
            Ok(Some(lease)) => lease,
            Ok(None) => continue,
            Err(error) => {
                log::warn!(
                    "could not acquire recovery lease for {}: {error}",
                    workspace.display()
                );
                continue;
            }
        };
        let metadata = read_metadata(&workspace.join(METADATA_FILE));
        snapshots.push(RecoverySnapshot {
            registry: root.to_path_buf(),
            workspace,
            document,
            project_name: metadata
                .as_ref()
                .and_then(|metadata| metadata.project_name.clone()),
            source_document: metadata.and_then(|metadata| metadata.source_document),
            modified: file.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
        drop(lease);
    }
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.modified));
    Ok(snapshots)
}

fn recovery_snapshots_from_known_roots() -> Result<Vec<RecoverySnapshot>, SessionError> {
    recovery_snapshots_from_roots(&known_recovery_roots())
}

fn known_recovery_roots() -> Vec<PathBuf> {
    let primary = recovery_dir();
    let fallback = fallback_recovery_dir();
    if primary == fallback {
        vec![primary]
    } else {
        vec![primary, fallback]
    }
}

fn recovery_snapshots_from_roots(roots: &[PathBuf]) -> Result<Vec<RecoverySnapshot>, SessionError> {
    let mut snapshots = Vec::new();
    let mut first_error = None;
    let mut readable_root = false;
    for root in roots {
        match recovery_snapshots_in(root) {
            Ok(mut found) => {
                readable_root = true;
                snapshots.append(&mut found);
            }
            Err(error) => {
                log::warn!(
                    "could not inspect recovery registry {}: {error}",
                    root.display()
                );
                first_error.get_or_insert(error);
            }
        }
    }
    if !readable_root {
        return Err(first_error.expect("at least one recovery registry was inspected"));
    }
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.modified));
    Ok(snapshots)
}

fn validate_snapshot(snapshot: &RecoverySnapshot) -> Result<(), SessionError> {
    let expected_document = snapshot.workspace.join(AUTOSAVE_FILE);
    let document_metadata = fs::symlink_metadata(&snapshot.document).ok();
    if !registry_workspace(&snapshot.registry, &snapshot.workspace)
        || snapshot.document != expected_document
        || !document_metadata.as_ref().is_some_and(|metadata| {
            metadata.is_file() && !metadata.file_type().is_symlink() && !is_reparse_point(metadata)
        })
    {
        return Err(SessionError::RecoveryUnavailable(snapshot.document.clone()));
    }

    let registry = fs::canonicalize(&snapshot.registry)
        .map_err(|error| io_error(&snapshot.registry, error))?;
    let workspace = fs::canonicalize(&snapshot.workspace)
        .map_err(|error| io_error(&snapshot.workspace, error))?;
    let Some(workspace_name) = snapshot.workspace.file_name() else {
        return Err(SessionError::RecoveryUnavailable(snapshot.document.clone()));
    };
    let expected_workspace = registry.join(workspace_name);
    let document = fs::canonicalize(&snapshot.document)
        .map_err(|error| io_error(&snapshot.document, error))?;
    if workspace != expected_workspace || document != workspace.join(AUTOSAVE_FILE) {
        return Err(SessionError::RecoveryUnavailable(snapshot.document.clone()));
    }
    Ok(())
}

fn acquire_snapshot_lease(snapshot: &RecoverySnapshot) -> Result<File, SessionError> {
    validate_snapshot(snapshot)?;
    let lease = try_workspace_lease(&snapshot.workspace)?
        .ok_or_else(|| SessionError::RecoveryUnavailable(snapshot.document.clone()))?;
    // The entry can be replaced between its first validation and opening the lease. Validate
    // again while holding the lock, and keep the handle alive through recovery or deletion.
    validate_snapshot(snapshot)?;
    Ok(lease)
}

fn private_asset_paths(project: &Project) -> Vec<AssetPath> {
    let mut paths: Vec<AssetPath> = project
        .audio_sources
        .values()
        .map(|source| source.path.clone())
        .chain(project.soundfonts.values().map(|font| font.path.clone()))
        .collect();
    paths.extend(project.tracks.iter().filter_map(|track| {
        track
            .kind
            .as_singer()
            .and_then(|singer| singer.voice.as_ref())
            .map(|voice| voice.path.clone())
    }));
    paths
}

fn migrate_private_assets(project: &Project, from: &Path, to: &Path) -> Result<(), SessionError> {
    let completed = migrate_private_assets_with_cancel(project, from, to, &AtomicBool::new(false))?;
    debug_assert!(completed, "a local migration is never cancelled");
    Ok(())
}

fn migrate_private_assets_with_cancel(
    project: &Project,
    from: &Path,
    to: &Path,
    cancelled: &AtomicBool,
) -> Result<bool, SessionError> {
    let mut copied = std::collections::HashSet::new();
    for path in private_asset_paths(project) {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if !path.is_inside() {
            continue;
        }
        let Some(source) = path.resolve(Some(from)) else {
            continue;
        };
        let Some(destination) = path.resolve(Some(to)) else {
            continue;
        };
        if !source.is_file() || !copied.insert(destination.clone()) {
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::copy(&source, &destination).map_err(|error| io_error(&destination, error))?;
    }
    Ok(!cancelled.load(Ordering::Relaxed))
}

fn workspace_prepared_for_autosave(workspace: &Path) -> bool {
    workspace
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(WORKSPACE_PREFIX) || name.starts_with(UNREGISTERED_RECOVERY_PREFIX)
        })
}

fn file_fingerprint(path: &std::path::Path) -> Option<u64> {
    file_fingerprint_with_cancel(path, &AtomicBool::new(false)).ok()?
}

fn file_fingerprint_with_cancel(path: &Path, cancelled: &AtomicBool) -> Result<Option<u64>, ()> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut reader = BufReader::new(file);
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = DefaultHasher::new();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(());
        }
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(_) => return Ok(None),
        };
        if read == 0 {
            return Ok(Some(hasher.finish()));
        }
        hasher.write(&buffer[..read]);
    }
}

/// How long after the last write the document is written again, if it has changed.
///
/// Thirty seconds. The file is JSON in the kilobytes and is written to a scratch file and renamed,
/// so the cost of one is not the reason for the number; the reason is that it bounds how much of a
/// take, a drag or an arrangement can be lost to a power cut to something nobody would call a
/// session's work.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);

/// Everything the autosave policy looks at.
///
/// Gathered into one value so the decision can be a function with a test rather than a condition
/// buried in a poll loop, where the only way to check it would be to wait thirty seconds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AutosaveState {
    /// Whether the user has left the feature on.
    pub enabled: bool,
    /// Whether the document has a permanent user-chosen path.
    ///
    /// This is reported for a frontend explaining the state, but is not an autosave condition:
    /// every session has private working storage.
    pub has_path: bool,
    /// Whether it has changed since it was last written.
    pub dirty: bool,
    /// Whether a drag or another multi-step gesture is part way through.
    pub gesture_open: bool,
    /// Whether the file on disk has been changed by another writer since this session last
    /// read or wrote it.
    pub overwritten: bool,
    /// How long since the document was last written, by any means.
    pub since_last_save: Duration,
}

/// A complete recovery snapshot write captured on the session thread.
pub struct AutosaveJob {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    project: Project,
    source_document: Option<PathBuf>,
    current_workspace: PathBuf,
    promote_workspace: bool,
}

/// A private recovery generation waiting for validation and atomic publication.
pub struct AutosaveResult {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    current_workspace: PathBuf,
    workspace: PathBuf,
    document: auris_io::StagedProject,
    metadata: auris_io::StagedFile,
    replacement: Option<SessionWorkspace>,
}

/// A saved-document identity captured for an off-thread external-change check.
pub struct DiskWatchJob {
    owner: Arc<auris_core::PluginRegistry>,
    path: PathBuf,
    disk_stamp: SystemTime,
    disk_fingerprint: Option<u64>,
}

/// The outcome of an off-thread external-change check.
pub struct DiskWatchResult {
    owner: Arc<auris_core::PluginRegistry>,
    path: PathBuf,
    disk_stamp: SystemTime,
    disk_fingerprint: Option<u64>,
    modified: bool,
}

impl DiskWatchJob {
    /// Compares the captured saved-document identity with disk.
    ///
    /// Returns `None` when cancellation is observed while hashing a file whose timestamp did not
    /// change. Missing and unreadable files are not external edits; a later save reports them.
    pub fn run(self, cancelled: &AtomicBool) -> Option<DiskWatchResult> {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let modified = match fs::metadata(&self.path).and_then(|metadata| metadata.modified()) {
            Ok(current_stamp) if current_stamp != self.disk_stamp => true,
            Ok(_) => match self.disk_fingerprint {
                Some(saved) => match file_fingerprint_with_cancel(&self.path, cancelled) {
                    Ok(Some(current)) => saved != current,
                    Ok(None) => false,
                    Err(()) => return None,
                },
                None => false,
            },
            Err(_) => false,
        };
        Some(DiskWatchResult {
            owner: self.owner,
            path: self.path,
            disk_stamp: self.disk_stamp,
            disk_fingerprint: self.disk_fingerprint,
            modified,
        })
    }
}

/// A crash-recovery request captured without reading the abandoned workspace.
pub struct RecoveryJob {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    original_path: Option<PathBuf>,
    render_rate: f64,
    snapshot: RecoverySnapshot,
}

/// A fully loaded recovery document awaiting its short session-thread handoff.
pub struct RecoveryResult {
    owner: Arc<auris_core::PluginRegistry>,
    revision: u64,
    original_path: Option<PathBuf>,
    snapshot: RecoverySnapshot,
    source_lease: File,
    project: Project,
    assets: PreparedAssets,
    replacement: SessionWorkspace,
}

/// Deferred deletion of a recovery workspace that was successfully consumed.
pub struct RecoveryCleanupJob {
    snapshot: RecoverySnapshot,
    _lease: File,
}

/// Permanently removes one validated recovery workspace on a worker thread.
pub struct DiscardRecoveryJob {
    snapshot: RecoverySnapshot,
}

/// Removes recovery workspaces whose discard was committed but whose final deletion failed.
///
/// The known recovery registries are captured without filesystem access. [`Self::run`] performs
/// the directory scan and deletion, so a frontend can move all potentially slow work to a
/// background executor during startup.
pub struct RecoveryQuarantineCleanupJob {
    roots: Vec<PathBuf>,
}

impl AutosaveJob {
    /// Writes the snapshot away from the session thread, stopping between filesystem stages.
    pub fn run(self, cancelled: &AtomicBool) -> Result<Option<AutosaveResult>, SessionError> {
        let AutosaveJob {
            owner,
            revision,
            mut project,
            source_document,
            current_workspace,
            promote_workspace,
        } = self;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let replacement = if promote_workspace {
            let workspace = create_registered_workspace(&recovery_dir(), &fallback_recovery_dir())?;
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            migrate_private_assets(&project, &current_workspace, workspace.path())?;
            Some(workspace)
        } else {
            None
        };
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let workspace = replacement.as_ref().map_or_else(
            || current_workspace.clone(),
            |workspace| workspace.path().to_path_buf(),
        );
        let document_path = workspace.join(AUTOSAVE_FILE);
        let metadata_path = workspace.join(METADATA_FILE);
        let metadata_bytes = recovery_metadata_bytes(&project, source_document)?;
        let document = auris_io::stage_project(&document_path, &mut project)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let metadata = auris_io::stage_file_bytes(&metadata_path, &metadata_bytes)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(AutosaveResult {
            owner,
            revision,
            current_workspace,
            workspace,
            document,
            metadata,
            replacement,
        }))
    }
}

impl RecoveryJob {
    /// Loads, validates, copies, and decodes the recovery document without touching the session.
    pub fn run(self, cancelled: &AtomicBool) -> Result<Option<RecoveryResult>, SessionError> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let source_lease = acquire_snapshot_lease(&self.snapshot)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let project = auris_io::load_project(&self.snapshot.document)?;
        validate_snapshot(&self.snapshot)?;
        let replacement = create_recovery_workspace_in(&self.snapshot.registry)?;
        if !migrate_private_assets_with_cancel(
            &project,
            &self.snapshot.workspace,
            replacement.path(),
            cancelled,
        )? {
            return Ok(None);
        }
        write_snapshot(
            &replacement.path().join(AUTOSAVE_FILE),
            &project,
            self.snapshot.source_document.clone(),
        )?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(assets) = prepare_project_assets(
            &project,
            Some(replacement.path()),
            self.render_rate,
            cancelled,
        ) else {
            return Ok(None);
        };
        Ok(Some(RecoveryResult {
            owner: self.owner,
            revision: self.revision,
            original_path: self.original_path,
            snapshot: self.snapshot,
            source_lease,
            project,
            assets,
            replacement,
        }))
    }
}

impl RecoveryCleanupJob {
    /// Deletes the consumed workspace after revalidating its private filesystem identity.
    pub fn run(self) -> Result<(), SessionError> {
        validate_snapshot(&self.snapshot)?;
        fs::remove_dir_all(&self.snapshot.workspace)
            .map_err(|error| io_error(&self.snapshot.workspace, error))
    }
}

fn discard_recovery_with_cleanup(
    snapshot: RecoverySnapshot,
    cancelled: &AtomicBool,
    cleanup: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<Option<RecoverySnapshot>, SessionError> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let lease = acquire_snapshot_lease(&snapshot)?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }

    // Reserve a random sibling name through exclusive creation. Removing the empty reservation
    // leaves a destination for the atomic rename; a racing replacement can at worst make rename
    // fail while the validated source remains in place. Renaming a directory over a symlink moves
    // the link itself rather than traversing it.
    let reservation = tempfile::Builder::new()
        .prefix(DISCARD_QUARANTINE_PREFIX)
        .tempfile_in(&snapshot.registry)
        .map_err(|error| io_error(&snapshot.registry, error))?;
    let quarantine = reservation.path().to_path_buf();
    reservation
        .close()
        .map_err(|error| io_error(&quarantine, error))?;
    validate_snapshot(&snapshot)?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }

    // Windows will not rename a directory containing this byte-range-locked file. Keep the lease
    // through the final identity and cancellation checks, then release it immediately before the
    // atomic rename that becomes the exclusive claim. A competing recovery/discard either loses
    // that rename or finds the source gone during its mandatory post-lock validation.
    drop(lease);
    fs::rename(&snapshot.workspace, &quarantine)
        .map_err(|error| io_error(&snapshot.workspace, error))?;
    // The recovery entry ceased to exist at the rename above. Cancellation after that boundary
    // cannot honestly call the operation cancelled, and cleanup failure must not make the UI
    // claim that the still-valid old entry remains available.
    if let Err(error) = cleanup(&quarantine) {
        log::warn!(
            "discarded recovery snapshot but could not remove quarantine {}: {error}",
            quarantine.display()
        );
    }
    Ok(Some(snapshot))
}

impl DiscardRecoveryJob {
    /// Quarantines the exact abandoned workspace unless cancelled before the commit boundary.
    ///
    /// Once the workspace has been atomically renamed out of the recovery registry, deletion is
    /// committed. Later cancellation and best-effort quarantine cleanup cannot change the result.
    pub fn run(self, cancelled: &AtomicBool) -> Result<Option<RecoverySnapshot>, SessionError> {
        discard_recovery_with_cleanup(self.snapshot, cancelled, |path| fs::remove_dir_all(path))
    }
}

impl RecoveryQuarantineCleanupJob {
    /// Removes real `.discarded-*` directories immediately below the captured registries.
    ///
    /// Entries outside that namespace, symbolic links, and Windows reparse points are left
    /// untouched. Every registry and eligible entry is attempted; if one or more operations fail,
    /// the first error is returned after the remaining cleanup opportunities have been tried.
    pub fn run(self) -> Result<(), SessionError> {
        let mut first_error = None;
        for root in self.roots {
            if let Err(error) =
                cleanup_recovery_quarantines_in(&root, |path| fs::remove_dir_all(path))
            {
                if first_error.is_none() {
                    first_error = Some(error);
                } else {
                    log::warn!(
                        "could not clean another recovery registry {}: {error}",
                        root.display()
                    );
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Whether a recovery snapshot should be written now.
pub fn should_autosave(state: AutosaveState) -> bool {
    state.enabled
        && state.dirty
        && !state.gesture_open
        && state.since_last_save >= AUTOSAVE_INTERVAL
}

impl Session {
    /// Captures a due recovery write without hashing or writing the saved project.
    ///
    /// Unlike [`Self::autosave_state`], this hot-path check does not call
    /// [`Self::externally_modified`]. Autosave never overwrites the permanent document, so that
    /// hash belongs to the separately throttled external-change watcher.
    pub fn begin_autosave_job(&mut self) -> Result<Option<AutosaveJob>, SessionError> {
        let due = self.autosave
            && self.dirty
            && self.transaction.is_none()
            && self.last_save.elapsed() >= AUTOSAVE_INTERVAL;
        if !due {
            return Ok(None);
        }
        self.collect_hosted_state()?;
        // Only a successfully captured job consumes this due time. If hosted state cannot be
        // collected, the next UI tick must be allowed to retry instead of silently waiting a
        // second full interval.
        self.last_save = Instant::now();
        let mut project = self.project.clone();
        if let Some(folder) = self.path.as_deref().and_then(auris_io::project_folder) {
            make_recovery_paths_external(&mut project, folder);
        }
        Ok(Some(AutosaveJob {
            owner: Arc::clone(&self.registry),
            revision: self.revision,
            project,
            source_document: source_document(self.path.as_deref()),
            current_workspace: self.work_dir.path().to_path_buf(),
            promote_workspace: !self.work_dir.registered,
        }))
    }

    /// Publishes a completed recovery write if it still belongs to this live session.
    ///
    /// A promotion is adopted only at its captured revision: otherwise replacing the old scratch
    /// directory could discard a private asset imported while the worker was copying it. A stale
    /// or cancelled generation is dropped without touching the last published snapshot and makes
    /// dirty work immediately eligible for another attempt.
    pub fn continue_autosave(&mut self, result: AutosaveResult, cancelled: &AtomicBool) -> bool {
        let expected_workspace = result
            .replacement
            .as_ref()
            .map_or(self.work_dir.path(), SessionWorkspace::path);
        let current = Arc::ptr_eq(&self.registry, &result.owner)
            && self.autosave
            && self.dirty
            && self.revision == result.revision
            && self.work_dir.path() == result.current_workspace
            && expected_workspace == result.workspace
            && !cancelled.load(Ordering::Relaxed);
        if !current {
            // The staged generation owns only private, unpublished files. Dropping it cannot
            // remove the last accepted snapshot, unlike deleting `Autosave.auris` by name.
            drop(result);
            self.reschedule_autosave();
            return false;
        }

        let AutosaveResult {
            workspace,
            document,
            metadata,
            replacement,
            ..
        } = result;
        let destination = workspace.join(AUTOSAVE_FILE);
        if let Err(error) = document.publish() {
            log::warn!(
                "could not publish recovery snapshot {}: {error}",
                destination.display()
            );
            // Close staged handles before a promoted temporary workspace tries to remove itself;
            // Windows cannot delete their parent while either handle is open.
            drop(metadata);
            drop(replacement);
            self.reschedule_autosave();
            return false;
        }

        // The document rename is the commit boundary. Metadata is only a display aid and is
        // deliberately published second: before this point every error leaves the previous
        // complete snapshot untouched; after it, the new project is already recoverable.
        let metadata_path = workspace.join(METADATA_FILE);
        if let Err(error) = metadata.publish() {
            log::warn!(
                "published recovery document but could not update metadata {}: {error}",
                metadata_path.display()
            );
        }

        if let Some(replacement) = replacement {
            let previous = std::mem::replace(&mut self.work_dir, replacement);
            drop(previous);
        }
        true
    }

    /// Makes a cancelled or stale autosave eligible on the next poll while work is still dirty.
    pub fn reschedule_autosave(&mut self) {
        if self.dirty {
            self.last_save = Instant::now()
                .checked_sub(AUTOSAVE_INTERVAL)
                .unwrap_or_else(Instant::now);
        }
    }

    fn ensure_recovery_workspace(&mut self) -> Result<(), SessionError> {
        if workspace_prepared_for_autosave(self.work_dir.path()) {
            return Ok(());
        }
        let primary = recovery_dir();
        self.ensure_recovery_workspace_in(&primary, &fallback_recovery_dir())
    }

    fn ensure_recovery_workspace_in(
        &mut self,
        primary: &Path,
        fallback: &Path,
    ) -> Result<(), SessionError> {
        if workspace_prepared_for_autosave(self.work_dir.path()) {
            return Ok(());
        }
        let replacement = create_registered_workspace(primary, fallback)?;
        migrate_private_assets(&self.project, self.work_dir.path(), replacement.path())?;
        let previous = std::mem::replace(&mut self.work_dir, replacement);
        drop(previous);
        Ok(())
    }

    /// Recovery snapshots left by sessions that did not shut down normally, newest first.
    ///
    /// Listing does not parse the project document, so one damaged entry cannot hide the other
    /// recoverable work. [`Self::recover_autosave`] performs the full project validation when the
    /// user chooses one. A workspace whose process lease is still held is active, not crashed, and
    /// is omitted.
    pub fn recovery_snapshots() -> Result<Vec<RecoverySnapshot>, SessionError> {
        recovery_snapshots_from_known_roots()
    }

    /// Captures cleanup of logically discarded recovery workspaces for a background executor.
    ///
    /// Constructing the job only records the known registry paths and performs no filesystem I/O.
    pub fn begin_recovery_quarantine_cleanup() -> RecoveryQuarantineCleanupJob {
        RecoveryQuarantineCleanupJob {
            roots: known_recovery_roots(),
        }
    }

    /// Restores a recovery snapshot as an unsaved document.
    ///
    /// The permanent project recorded in [`RecoverySnapshot::source_document`] is never written
    /// or selected as this session's save path. Private audio is first copied into this session's
    /// own replacement recovery workspace and a fresh recovery snapshot is written there; only
    /// then is the abandoned workspace removed. A second crash during recovery therefore leaves
    /// at least one complete copy, even when the receiving session normally has autosave off.
    ///
    /// The caller must resolve unsaved edits in the current document first. Missing referenced
    /// assets are returned in the same form as [`Self::open`](Session::open). The process lease is
    /// acquired again before reading, so an entry another process claimed after listing is
    /// refused.
    pub fn recover_autosave(
        &mut self,
        snapshot: &RecoverySnapshot,
    ) -> Result<Vec<PathBuf>, SessionError> {
        let result = self
            .begin_recover_autosave(snapshot)?
            .run(&AtomicBool::new(false))?
            .expect("a local recovery is not cancelled");
        let (missing, cleanup) = self
            .continue_recover_autosave(result)
            .expect("a local recovery keeps the same session identity and revision");
        if let Err(error) = cleanup.run() {
            log::warn!("recovered project but kept its source workspace: {error}");
        }
        Ok(missing)
    }

    /// Captures a recovery request without reading or copying the abandoned workspace.
    pub fn begin_recover_autosave(
        &self,
        snapshot: &RecoverySnapshot,
    ) -> Result<RecoveryJob, SessionError> {
        if self.transaction.is_some() {
            return Err(SessionError::EditInProgress);
        }
        if self.dirty {
            return Err(SessionError::RecoveryWouldDiscardChanges);
        }
        Ok(RecoveryJob {
            owner: Arc::clone(&self.registry),
            revision: self.revision,
            original_path: self.path.clone(),
            render_rate: self.engine.sample_rate(),
            snapshot: snapshot.clone(),
        })
    }

    /// Adopts a fully prepared recovery result if the receiving document is still unchanged.
    pub fn continue_recover_autosave(
        &mut self,
        result: RecoveryResult,
    ) -> Option<(Vec<PathBuf>, RecoveryCleanupJob)> {
        if !Arc::ptr_eq(&self.registry, &result.owner)
            || self.revision != result.revision
            || self.path != result.original_path
            || self.dirty
            || self.transaction.is_some()
            || self.engine.sample_rate() != result.assets.render_rate
        {
            return None;
        }
        let RecoveryResult {
            snapshot,
            source_lease,
            project,
            assets,
            replacement,
            ..
        } = result;
        let previous_workspace = std::mem::replace(&mut self.work_dir, replacement);
        drop(previous_workspace);

        self.history.clear();
        self.clear_sources();
        self.sound_scope = crate::transient_id::transient_id("session");
        self.path = None;
        self.disk_stamp = None;
        self.disk_fingerprint = None;
        self.armed.clear();
        self.monitored.clear();
        self.publish_monitors();
        self.close_input_if_idle();
        self.hosted.clear();
        self.vst3.clear();

        // There is no saved document for an Undo to compare with. A deliberately unequal
        // internal baseline keeps the recovered state dirty when the first edit is undone.
        let mut unsaved_baseline = project.clone();
        unsaved_baseline.saved_by.push('\0');
        self.saved_project = unsaved_baseline.clone();
        self.saved_edit_project = unsaved_baseline;
        self.adopt_project(project);
        let missing = self.install_prepared_assets(assets);
        self.install_shipped_fonts();
        self.rebuild_graph();
        if self.realign_automation() {
            self.rebuild_graph();
        }
        self.publish_loop();
        self.dirty = true;
        self.last_save = Instant::now();
        Some((
            missing,
            RecoveryCleanupJob {
                snapshot,
                _lease: source_lease,
            },
        ))
    }

    /// Permanently deletes one recovery snapshot without opening it.
    ///
    /// The entry's private identity is validated before its workspace is recursively removed, so
    /// a path supplied through any other API cannot turn this into an arbitrary-directory delete.
    /// Its process lease is also acquired again, so a workspace claimed after listing is never
    /// removed from underneath its owner.
    pub fn discard_recovery(snapshot: &RecoverySnapshot) -> Result<(), SessionError> {
        Self::begin_discard_recovery(snapshot)
            .run(&AtomicBool::new(false))?
            .expect("a local discard is not cancelled");
        Ok(())
    }

    /// Captures a recovery deletion without inspecting or mutating the filesystem.
    pub fn begin_discard_recovery(snapshot: &RecoverySnapshot) -> DiscardRecoveryJob {
        DiscardRecoveryJob {
            snapshot: snapshot.clone(),
        }
    }

    /// Whether the document is being snapshotted as it changes.
    pub fn autosave_enabled(&self) -> bool {
        self.autosave
    }

    /// Turns autosaving on or off.
    ///
    /// Turning it on does not save anything immediately; the next tick that finds the document
    /// changed and the interval elapsed does.
    pub fn set_autosave(&mut self, enabled: bool) {
        self.autosave = enabled;
    }

    /// What the policy is looking at right now, for a frontend that wants to explain itself.
    pub fn autosave_state(&self) -> AutosaveState {
        AutosaveState {
            enabled: self.autosave,
            has_path: self.path.is_some(),
            dirty: self.dirty,
            gesture_open: self.transaction.is_some(),
            overwritten: self.externally_modified(),
            since_last_save: self.last_save.elapsed(),
        }
    }

    /// Writes a recovery snapshot if the policy says it is time.
    ///
    /// `None` means nothing was attempted, which is the answer almost every time it is asked.
    /// Call it from whatever the frontend already runs each frame — it is a handful of
    /// comparisons until the moment it is not.
    ///
    /// Deliberately not part of [`Session::poll`]. That is housekeeping and this writes to
    /// somebody's disk, and a method whose name promises the first should never quietly do the
    /// second.
    pub fn autosave(&mut self) -> Option<Result<(), SessionError>> {
        if !should_autosave(self.autosave_state()) {
            return None;
        }
        // Stamped whether or not the write succeeds: a disk that is refusing should be retried at
        // the same interval as everything else, not on every frame.
        self.last_save = Instant::now();
        Some(
            self.ensure_recovery_workspace()
                .and_then(|()| self.save_autosave()),
        )
    }

    /// The private recovery document autosave writes.
    pub fn autosave_path(&self) -> std::path::PathBuf {
        self.work_dir.path().join(AUTOSAVE_FILE)
    }

    /// Writes the current project to the private cache without changing saved/dirty state.
    ///
    /// An inside asset from a saved project belongs beside the real document, not beside this
    /// snapshot. Its recovery reference is made absolute in the clone so opening the snapshot
    /// still finds the audio and voice models without copying assets on each autosave.
    fn save_autosave(&mut self) -> Result<(), SessionError> {
        self.collect_hosted_state()?;
        let mut recovery = self.project.clone();
        if let Some(folder) = self.path.as_deref().and_then(auris_io::project_folder) {
            make_recovery_paths_external(&mut recovery, folder);
        }
        write_snapshot(
            &self.autosave_path(),
            &recovery,
            source_document(self.path.as_deref()),
        )
    }

    /// Restarts the autosave clock. Called by every path that writes the document — and by
    /// `open`, which is the other way this session and the file come to agree.
    ///
    /// The disk stamp is taken here because this is that agreement's one funnel: whatever the
    /// file's modification time is at this moment is *ours*, and a different one later means
    /// another writer — see [`Session::externally_modified`].
    pub(super) fn mark_saved(&mut self) {
        let disk_stamp = self
            .path
            .as_deref()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|meta| meta.modified().ok());
        let disk_fingerprint = self.path.as_deref().and_then(file_fingerprint);
        self.finish_mark_saved(disk_stamp, disk_fingerprint);
    }

    /// Finishes a worker-side save without rereading the newly published document on this thread.
    pub(super) fn mark_saved_from_worker(
        &mut self,
        disk_stamp: Option<std::time::SystemTime>,
        disk_fingerprint: u64,
    ) {
        self.finish_mark_saved(disk_stamp, Some(disk_fingerprint));
    }

    fn finish_mark_saved(
        &mut self,
        disk_stamp: Option<std::time::SystemTime>,
        disk_fingerprint: Option<u64>,
    ) {
        self.last_save = Instant::now();
        // The real document and memory agree again (or a fresh document was deliberately
        // started), so an older recovery snapshot must not be mistaken for newer work.
        let _ = std::fs::remove_file(self.autosave_path());
        let _ = std::fs::remove_file(self.work_dir.path().join(METADATA_FILE));
        self.saved_project = self.project.clone();
        self.saved_edit_project = self.project.clone();
        self.disk_stamp = disk_stamp;
        self.disk_fingerprint = disk_fingerprint;
    }

    /// Captures an external-change check without touching the filesystem.
    ///
    /// The returned job may be run on a worker and handed back through
    /// [`Self::continue_disk_watch`]. Unsaved edits deliberately do not invalidate the job:
    /// the saved disk baseline remains the same until a document is opened or published.
    pub fn begin_disk_watch(&self) -> Option<DiskWatchJob> {
        Some(DiskWatchJob {
            owner: Arc::clone(&self.registry),
            path: self.path.clone()?,
            disk_stamp: self.disk_stamp?,
            disk_fingerprint: self.disk_fingerprint,
        })
    }

    /// Accepts an external-change result if the saved disk baseline is still the one captured.
    ///
    /// `None` means an open, save, or document replacement made the worker result stale.
    pub fn continue_disk_watch(&self, result: DiskWatchResult) -> Option<bool> {
        (Arc::ptr_eq(&self.registry, &result.owner)
            && self.path.as_ref() == Some(&result.path)
            && self.disk_stamp == Some(result.disk_stamp)
            && self.disk_fingerprint == result.disk_fingerprint)
            .then_some(result.modified)
    }

    /// Whether the file on disk is no longer the one this session last read or wrote.
    ///
    /// `true` means another writer has been at it — the MCP door, a sync service, anything —
    /// and what this session would save is based on a version that is no longer there.
    /// A file that cannot be examined (deleted, unreadable) answers `false`: that is a
    /// different problem, and the next save will say so in its own words.
    pub fn externally_modified(&self) -> bool {
        let (Some(path), Some(stamp)) = (self.path.as_deref(), self.disk_stamp) else {
            return false;
        };
        match std::fs::metadata(path).and_then(|meta| meta.modified()) {
            Ok(now) => {
                now != stamp
                    || self
                        .disk_fingerprint
                        .zip(file_fingerprint(path))
                        .is_some_and(|(saved, current)| saved != current)
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosave_off_uses_an_ordinary_temporary_workspace() {
        let session = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        let name = session
            .work_dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy();

        assert!(name.starts_with("auris-studio-"));
        assert!(!name.starts_with(WORKSPACE_PREFIX));
    }

    #[test]
    fn an_unwritable_primary_registry_falls_back_without_blocking_startup() {
        let scratch = tempfile::tempdir().unwrap();
        let blocked_primary = scratch.path().join("primary-is-a-file");
        std::fs::write(&blocked_primary, b"not a directory").unwrap();
        let fallback = scratch.path().join("fallback");

        let workspace = create_registered_workspace(&blocked_primary, &fallback).unwrap();

        assert_eq!(workspace.path().parent(), Some(fallback.as_path()));
        assert!(
            workspace
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(WORKSPACE_PREFIX)
        );
    }

    #[test]
    fn unavailable_recovery_registries_still_allow_anonymous_scratch_storage() {
        let scratch = tempfile::tempdir().unwrap();
        let blocked_primary = scratch.path().join("primary-is-a-file");
        let blocked_fallback = scratch.path().join("fallback-is-a-file");
        std::fs::write(&blocked_primary, b"not a directory").unwrap();
        std::fs::write(&blocked_fallback, b"not a directory").unwrap();

        let workspace = create_registered_workspace(&blocked_primary, &blocked_fallback).unwrap();
        let name = workspace.path().file_name().unwrap().to_string_lossy();

        assert!(name.starts_with(UNREGISTERED_RECOVERY_PREFIX));
        assert!(!name.starts_with(WORKSPACE_PREFIX));
    }

    #[test]
    fn enabling_autosave_promotes_private_assets_into_a_registered_workspace() {
        let primary = tempfile::tempdir().unwrap();
        let fallback = tempfile::tempdir().unwrap();
        let mut session = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        let old_workspace = session.work_dir.path().to_path_buf();
        let old_asset = old_workspace.join("Audio").join("take.wav");
        std::fs::create_dir_all(old_asset.parent().unwrap()).unwrap();
        std::fs::write(&old_asset, b"private audio").unwrap();
        session.project.add_audio_source(
            "Take",
            AssetPath::inside("Audio/take.wav"),
            1,
            48_000.0,
            1,
        );
        session.set_autosave(true);

        session
            .ensure_recovery_workspace_in(primary.path(), fallback.path())
            .unwrap();

        assert_eq!(session.work_dir.path().parent(), Some(primary.path()));
        assert_eq!(
            std::fs::read(session.work_dir.path().join("Audio").join("take.wav")).unwrap(),
            b"private audio"
        );
        assert!(!old_workspace.exists());
    }

    fn move_workspace_into(session: &mut Session, registry: &std::path::Path) {
        let replacement = create_recovery_workspace_in(registry).unwrap();
        let previous = std::mem::replace(&mut session.work_dir, replacement);
        drop(previous);
    }

    fn leave_workspace_behind(session: &mut Session, replacement_root: &std::path::Path) {
        let replacement = create_recovery_workspace_in(replacement_root).unwrap();
        let mut crashed = std::mem::replace(&mut session.work_dir, replacement);
        crashed.keep_for_recovery();
    }

    /// A document that would be saved: on, saved before, changed, idle, and overdue.
    fn ready() -> AutosaveState {
        AutosaveState {
            enabled: true,
            has_path: true,
            dirty: true,
            gesture_open: false,
            overwritten: false,
            since_last_save: AUTOSAVE_INTERVAL,
        }
    }

    #[test]
    fn another_writers_version_does_not_block_the_separate_snapshot() {
        assert!(should_autosave(AutosaveState {
            overwritten: true,
            ..ready()
        }));
    }

    #[test]
    fn a_document_that_has_changed_is_written_once_the_interval_is_up() {
        assert!(should_autosave(ready()));
    }

    fn due_autosave_session() -> Session {
        let mut options = crate::SessionOptions::headless();
        options.autosave = true;
        let mut session = Session::new(options).unwrap();
        session.add_default_instrument_track("Changed").unwrap();
        session.last_save = Instant::now()
            .checked_sub(AUTOSAVE_INTERVAL)
            .unwrap_or_else(Instant::now);
        session
    }

    fn workspace_entry_names(path: &Path) -> std::collections::BTreeSet<std::ffi::OsString> {
        std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    }

    #[test]
    fn stale_worker_preserves_the_last_complete_snapshot_and_is_due_immediately() {
        let mut session = due_autosave_session();
        let track = session.project.tracks[0].id;
        session.save_autosave().unwrap();
        let previous_document = std::fs::read(session.autosave_path()).unwrap();
        let metadata_path = session.work_dir.path().join(METADATA_FILE);
        let previous_metadata = std::fs::read(&metadata_path).unwrap();
        let workspace = session.work_dir.path().to_path_buf();
        let previous_entries = workspace_entry_names(&workspace);

        session.rename_track(track, "Worker generation").unwrap();
        session.reschedule_autosave();
        let job = session.begin_autosave_job().unwrap().unwrap();
        session.rename_track(track, "Changed again").unwrap();
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace_entry_names(&workspace).len(),
            previous_entries.len() + 2,
            "one private document and metadata stage belong to the pending generation"
        );

        assert!(!session.continue_autosave(result, &AtomicBool::new(false)));
        assert_eq!(workspace_entry_names(&workspace), previous_entries);
        assert_eq!(
            std::fs::read(session.autosave_path()).unwrap(),
            previous_document
        );
        assert_eq!(std::fs::read(metadata_path).unwrap(), previous_metadata);
        assert!(session.begin_autosave_job().unwrap().is_some());
    }

    #[test]
    fn a_completed_worker_cancelled_before_continue_preserves_the_last_complete_snapshot() {
        let mut session = due_autosave_session();
        let track = session.project.tracks[0].id;
        session.save_autosave().unwrap();
        let previous_document = std::fs::read(session.autosave_path()).unwrap();
        let metadata_path = session.work_dir.path().join(METADATA_FILE);
        let previous_metadata = std::fs::read(&metadata_path).unwrap();
        let workspace = session.work_dir.path().to_path_buf();
        let previous_entries = workspace_entry_names(&workspace);

        session.rename_track(track, "Cancelled generation").unwrap();
        session.reschedule_autosave();
        let job = session.begin_autosave_job().unwrap().unwrap();
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace_entry_names(&workspace).len(),
            previous_entries.len() + 2
        );

        assert!(!session.continue_autosave(result, &AtomicBool::new(true)));

        assert_eq!(workspace_entry_names(&workspace), previous_entries);
        assert_eq!(
            std::fs::read(session.autosave_path()).unwrap(),
            previous_document
        );
        assert_eq!(std::fs::read(metadata_path).unwrap(), previous_metadata);
        assert!(session.begin_autosave_job().unwrap().is_some());
    }

    #[test]
    fn a_worker_error_preserves_the_last_complete_snapshot() {
        let mut session = due_autosave_session();
        let track = session.project.tracks[0].id;
        session.save_autosave().unwrap();
        let previous_document = std::fs::read(session.autosave_path()).unwrap();
        let metadata_path = session.work_dir.path().join(METADATA_FILE);
        let previous_metadata = std::fs::read(&metadata_path).unwrap();
        let previous_entries = workspace_entry_names(session.work_dir.path());

        let clip = session
            .project
            .add_midi_clip(
                track,
                "Invalid loop",
                auris_core::time::Ticks::ZERO,
                auris_core::time::Ticks(1),
            )
            .unwrap();
        let clip = session.project.tracks[0]
            .kind
            .note_clips_mut()
            .unwrap()
            .iter_mut()
            .find(|candidate| candidate.id == clip)
            .unwrap();
        clip.notes.push(auris_core::Note::new(
            60,
            auris_core::time::Ticks::ZERO,
            auris_core::time::Ticks(1),
        ));
        clip.loop_end = auris_core::time::Ticks(i64::MAX);
        session.reschedule_autosave();
        let job = session.begin_autosave_job().unwrap().unwrap();

        assert!(job.run(&AtomicBool::new(false)).is_err());
        assert_eq!(
            std::fs::read(session.autosave_path()).unwrap(),
            previous_document
        );
        assert_eq!(std::fs::read(metadata_path).unwrap(), previous_metadata);
        assert_eq!(
            workspace_entry_names(session.work_dir.path()),
            previous_entries
        );
    }

    #[test]
    fn a_current_worker_atomically_replaces_an_existing_snapshot_and_removes_its_stages() {
        let mut session = due_autosave_session();
        let track = session.project.tracks[0].id;
        session.save_autosave().unwrap();
        let workspace = session.work_dir.path().to_path_buf();
        let previous_entries = workspace_entry_names(&workspace);
        session.rename_track(track, "Accepted generation").unwrap();
        session.reschedule_autosave();
        let job = session.begin_autosave_job().unwrap().unwrap();
        let result = job
            .run(&AtomicBool::new(false))
            .unwrap()
            .expect("the worker was not cancelled");
        assert_eq!(
            workspace_entry_names(&workspace).len(),
            previous_entries.len() + 2
        );

        assert!(session.continue_autosave(result, &AtomicBool::new(false)));

        assert_eq!(workspace_entry_names(&workspace), previous_entries);
        assert_eq!(
            auris_io::load_project(&session.autosave_path())
                .unwrap()
                .tracks[0]
                .name,
            "Accepted generation"
        );
        assert!(session.work_dir.path().join(METADATA_FILE).is_file());
    }

    #[test]
    fn completed_old_snapshot_cannot_reappear_after_starting_a_clean_document() {
        let mut session = due_autosave_session();
        let job = session.begin_autosave_job().unwrap().unwrap();
        session.new_project();
        let workspace = session.work_dir.path().to_path_buf();
        let previous_entries = workspace_entry_names(&workspace);
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace_entry_names(&workspace).len(),
            previous_entries.len() + 2,
            "the race is exercised after the old worker stages its generation"
        );

        assert!(!session.continue_autosave(result, &AtomicBool::new(false)));
        assert_eq!(workspace_entry_names(&workspace), previous_entries);
        assert!(!workspace.join(AUTOSAVE_FILE).exists());
        assert!(!session.is_dirty());
    }

    #[test]
    fn cancelled_worker_is_due_again_without_waiting_another_interval() {
        let mut session = due_autosave_session();
        let job = session.begin_autosave_job().unwrap().unwrap();
        assert!(job.run(&AtomicBool::new(true)).unwrap().is_none());

        session.reschedule_autosave();

        assert!(session.begin_autosave_job().unwrap().is_some());
    }

    #[test]
    fn a_worker_finishing_after_manual_save_cannot_restore_a_recovery_snapshot() {
        let scratch = tempfile::tempdir().unwrap();
        let saved = scratch.path().join("Saved.auris");
        let mut session = due_autosave_session();
        let job = session.begin_autosave_job().unwrap().unwrap();
        session.save(&saved).unwrap();
        let workspace = session.work_dir.path().to_path_buf();
        let previous_entries = workspace_entry_names(&workspace);
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace_entry_names(&workspace).len(),
            previous_entries.len() + 2
        );

        assert!(!session.continue_autosave(result, &AtomicBool::new(false)));
        assert_eq!(workspace_entry_names(&workspace), previous_entries);
        assert!(!workspace.join(AUTOSAVE_FILE).exists());
        assert!(!session.is_dirty());
    }

    #[test]
    fn a_worker_finishing_after_open_cannot_replace_the_open_documents_recovery_state() {
        let scratch = tempfile::tempdir().unwrap();
        let other_path = scratch.path().join("Other.auris");
        let mut other = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        other
            .add_default_instrument_track("Other document")
            .unwrap();
        other.save(&other_path).unwrap();

        let mut session = due_autosave_session();
        let job = session.begin_autosave_job().unwrap().unwrap();
        session.open(&other_path).unwrap();
        let opened = session.project().clone();
        let workspace = session.work_dir.path().to_path_buf();
        let previous_entries = workspace_entry_names(&workspace);
        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace_entry_names(&workspace).len(),
            previous_entries.len() + 2
        );

        assert!(!session.continue_autosave(result, &AtomicBool::new(false)));
        assert_eq!(workspace_entry_names(&workspace), previous_entries);
        assert!(!workspace.join(AUTOSAVE_FILE).exists());
        assert_eq!(session.project(), &opened);
        assert!(!session.is_dirty());
    }

    #[test]
    fn an_unsaved_document_is_snapshotted_without_choosing_its_permanent_path() {
        assert!(should_autosave(AutosaveState {
            has_path: false,
            ..ready()
        }));
    }

    #[test]
    fn autosave_uses_the_cache_and_keeps_the_document_dirty() {
        let root = std::env::temp_dir().join(format!(
            "auris-session-autosave-destination-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut session =
            crate::Session::new(crate::SessionOptions::headless()).expect("a headless session");
        let saved = session.save_as(&root.join("Song.auris")).unwrap().document;
        session.add_default_instrument_track("Changed").unwrap();

        let original = std::fs::read(&saved).unwrap();
        session.save_autosave().unwrap();

        assert_eq!(std::fs::read(&saved).unwrap(), original);
        assert!(session.autosave_path().is_file());
        assert_ne!(session.autosave_path(), saved);
        assert!(session.is_dirty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_recovery_snapshot_keeps_project_local_singer_voices_resolvable() {
        let scratch = super::super::fixtures::Scratch::new("autosave-singer-voice");
        let mut session = super::super::fixtures::session();
        let saved = session
            .save_as(&scratch.join("Song.auris"))
            .unwrap()
            .document;
        let voices = saved.parent().unwrap().join("Voices");
        std::fs::create_dir_all(&voices).unwrap();
        let voice_file = voices.join("voice.onnx");
        // Only path recovery is under test; no model runtime or large voice fixture is needed.
        std::fs::write(&voice_file, b"voice fixture").unwrap();
        let track = session.add_singer_track("Voice");
        let voice_path = auris_core::AssetPath::inside("Voices/voice.onnx");
        session
            .project
            .track_mut(track)
            .unwrap()
            .kind
            .as_singer_mut()
            .unwrap()
            .voice = Some(auris_core::SingerVoice {
            path: voice_path.clone(),
            name: "Test Voice".into(),
            consonants: None,
            levels: None,
            speaker: None,
        });
        let original = session.project.clone();
        let saved_bytes = std::fs::read(&saved).unwrap();

        session.save_autosave().unwrap();

        let snapshot = session.autosave_path();
        let recovery = auris_io::load_project(&snapshot).unwrap();
        let recovered_voice = recovery
            .track(track)
            .unwrap()
            .kind
            .as_singer()
            .unwrap()
            .voice
            .as_ref()
            .unwrap();
        let resolved = recovered_voice
            .path
            .resolve(auris_io::project_folder(&snapshot))
            .unwrap();
        assert_eq!(resolved, voice_file);
        assert_eq!(std::fs::read(resolved).unwrap(), b"voice fixture");
        assert_eq!(session.project, original);
        assert_eq!(
            session.singer_voice(track).unwrap().unwrap().path,
            voice_path
        );
        assert!(session.is_dirty());
        assert_eq!(std::fs::read(saved).unwrap(), saved_bytes);
    }

    #[test]
    fn nothing_is_written_part_way_through_a_gesture() {
        // A drag is one undo step and, until it ends, a document caught halfway through a change
        // nobody has finished making.
        assert!(!should_autosave(AutosaveState {
            gesture_open: true,
            ..ready()
        }));
    }

    #[test]
    fn an_unchanged_document_is_left_alone() {
        // Otherwise a project left open overnight would be rewritten twice a minute for no
        // reason, and its modification time would say it was worked on all night.
        assert!(!should_autosave(AutosaveState {
            dirty: false,
            ..ready()
        }));
    }

    #[test]
    fn the_interval_is_a_floor_rather_than_a_target() {
        let mut state = ready();
        state.since_last_save = AUTOSAVE_INTERVAL - Duration::from_millis(1);
        assert!(!should_autosave(state));

        state.since_last_save = AUTOSAVE_INTERVAL * 10;
        assert!(should_autosave(state), "an overdue save still happens");
    }

    #[test]
    fn switching_it_off_switches_it_off() {
        assert!(!should_autosave(AutosaveState {
            enabled: false,
            ..ready()
        }));
    }

    /// The stamp agrees after every read and write of the file, and disagrees — visibly —
    /// the moment another writer touches it.
    #[test]
    fn another_writer_is_noticed_and_a_save_of_our_own_is_not() {
        let root = std::env::temp_dir().join(format!("auris-session-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session =
            crate::Session::new(crate::SessionOptions::headless()).expect("a headless session");
        session.save_as(&root.join("Watched.auris")).unwrap();
        assert!(
            !session.externally_modified(),
            "the file just written is our own"
        );

        // Another writer, played by a bumped modification time — how it looks from here,
        // however the bytes changed. Set explicitly rather than written and slept for,
        // because a fast filesystem gives two writes in one timestamp.
        let document = session.path().unwrap().to_path_buf();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&document)
            .unwrap();
        file.set_modified(std::time::SystemTime::now() + Duration::from_secs(2))
            .unwrap();
        drop(file);
        assert!(session.externally_modified(), "the other writer shows");
        assert!(
            session.autosave_state().overwritten,
            "and the autosave policy sees it"
        );

        // Saving by hand is the deliberate act that takes the file back.
        session.save_in_place().unwrap();
        assert!(!session.externally_modified(), "ours again");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn another_writer_is_noticed_when_the_timestamp_does_not_move() {
        let root =
            std::env::temp_dir().join(format!("auris-session-same-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session =
            crate::Session::new(crate::SessionOptions::headless()).expect("a headless session");
        session.save_as(&root.join("Watched.auris")).unwrap();
        let document = session.path().unwrap().to_path_buf();
        let stamp = session.disk_stamp.unwrap();
        let mut bytes = std::fs::read(&document).unwrap();
        let index = bytes.iter().position(|byte| *byte == b' ').unwrap_or(0);
        bytes[index] = if bytes[index] == b' ' { b'\t' } else { b' ' };
        std::fs::write(&document, bytes).unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&document)
            .unwrap();
        file.set_modified(stamp).unwrap();
        drop(file);

        assert!(session.externally_modified());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detached_disk_watch_hashes_same_stamp_changes_and_rejects_a_stale_document() {
        let scratch = tempfile::tempdir().unwrap();
        let mut session =
            crate::Session::new(crate::SessionOptions::headless()).expect("a headless session");
        session
            .save_as(&scratch.path().join("Watched.auris"))
            .unwrap();
        let document = session.path().unwrap().to_path_buf();
        let stamp = session.disk_stamp.unwrap();
        let job = session.begin_disk_watch().unwrap();
        let mut bytes = std::fs::read(&document).unwrap();
        let index = bytes.iter().position(|byte| *byte == b' ').unwrap_or(0);
        bytes[index] = if bytes[index] == b' ' { b'\t' } else { b' ' };
        std::fs::write(&document, bytes).unwrap();
        File::options()
            .write(true)
            .open(&document)
            .unwrap()
            .set_modified(stamp)
            .unwrap();

        let result = std::thread::spawn(move || job.run(&AtomicBool::new(false)))
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(session.continue_disk_watch(result), Some(true));

        let stale = session.begin_disk_watch().unwrap();
        session.new_project();
        let stale = stale.run(&AtomicBool::new(false)).unwrap();
        assert_eq!(session.continue_disk_watch(stale), None);
    }

    #[test]
    fn a_cancelled_disk_watch_never_produces_a_handoff() {
        let scratch = tempfile::tempdir().unwrap();
        let mut session =
            crate::Session::new(crate::SessionOptions::headless()).expect("a headless session");
        session
            .save_as(&scratch.path().join("Watched.auris"))
            .unwrap();

        assert!(
            session
                .begin_disk_watch()
                .unwrap()
                .run(&AtomicBool::new(true))
                .is_none()
        );
    }

    #[test]
    fn a_crashed_sessions_snapshot_is_discovered_by_the_next_session() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let saved_root = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        let saved = crashed
            .save_as(&saved_root.path().join("Song.auris"))
            .unwrap()
            .document;
        crashed.project.name = "Unfinished chorus".into();
        crashed.add_default_instrument_track("Lead").unwrap();
        crashed.save_autosave().unwrap();
        let document = crashed.autosave_path();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);

        let snapshots = recovery_snapshots_in(registry.path()).unwrap();

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].project_name(), Some("Unfinished chorus"));
        assert_eq!(
            snapshots[0].source_document(),
            Some(std::path::absolute(saved).unwrap().as_path())
        );
        assert_eq!(snapshots[0].document(), document);
    }

    #[test]
    fn a_panicking_session_leaves_its_snapshot_for_recovery() {
        let registry = tempfile::tempdir().unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut session = crate::Session::new(crate::SessionOptions::headless()).unwrap();
            move_workspace_into(&mut session, registry.path());
            session.project.name = "Interrupted edit".into();
            session.add_default_instrument_track("Lead").unwrap();
            session.save_autosave().unwrap();

            panic!("simulate an application panic after autosave");
        }));

        assert!(result.is_err());
        let snapshots = recovery_snapshots_in(registry.path()).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].project_name(), Some("Interrupted edit"));
        Session::discard_recovery(&snapshots[0]).unwrap();
    }

    #[test]
    fn a_normally_dropped_session_removes_its_snapshot() {
        let registry = tempfile::tempdir().unwrap();
        let workspace;
        {
            let mut session = crate::Session::new(crate::SessionOptions::headless()).unwrap();
            move_workspace_into(&mut session, registry.path());
            session
                .add_default_instrument_track("Discarded edit")
                .unwrap();
            session.save_autosave().unwrap();
            workspace = session.work_dir.path().to_path_buf();
        }

        assert!(!workspace.exists());
        assert!(recovery_snapshots_in(registry.path()).unwrap().is_empty());
    }

    #[test]
    fn a_broken_primary_registry_does_not_hide_the_fallback_registry() {
        let scratch = tempfile::tempdir().unwrap();
        let blocked_primary = scratch.path().join("primary-is-a-file");
        std::fs::write(&blocked_primary, b"not a directory").unwrap();
        let fallback = scratch.path().join("fallback");
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, &fallback);
        crashed.add_default_instrument_track("Lead").unwrap();
        crashed.save_autosave().unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);

        let snapshots = recovery_snapshots_from_roots(&[blocked_primary, fallback]).unwrap();

        assert_eq!(snapshots.len(), 1);
    }

    #[test]
    fn an_active_sessions_workspace_is_not_offered_for_recovery() {
        let registry = tempfile::tempdir().unwrap();
        let mut active = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut active, registry.path());
        active.add_default_instrument_track("In progress").unwrap();
        active.save_autosave().unwrap();

        assert!(
            try_workspace_lease(active.work_dir.path())
                .unwrap()
                .is_none(),
            "a second handle in this process sees the same contention as another process"
        );
        assert!(recovery_snapshots_in(registry.path()).unwrap().is_empty());

        // Closing a process releases all of its file handles. Dropping the owning handle here is
        // the same OS-level transition, without needing a child test process.
        drop(active.work_dir._lease.take());
        assert_eq!(recovery_snapshots_in(registry.path()).unwrap().len(), 1);
    }

    #[test]
    fn a_snapshot_reacquired_after_listing_cannot_be_recovered_or_discarded() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed.add_default_instrument_track("Recovered").unwrap();
        crashed.save_autosave().unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);
        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();
        let active_lease = try_workspace_lease(&snapshot.workspace)
            .unwrap()
            .expect("the abandoned workspace can be acquired");
        let mut current = crate::Session::new(crate::SessionOptions::headless()).unwrap();

        let recover_error = current.recover_autosave(&snapshot).unwrap_err();
        let discard_error = Session::discard_recovery(&snapshot).unwrap_err();

        assert!(matches!(
            recover_error,
            SessionError::RecoveryUnavailable(_)
        ));
        assert!(matches!(
            discard_error,
            SessionError::RecoveryUnavailable(_)
        ));
        assert!(snapshot.document().is_file());

        drop(active_lease);
        Session::discard_recovery(&snapshot).unwrap();
    }

    #[test]
    fn recovery_moves_private_assets_to_the_new_sessions_workspace() {
        let abandoned_registry = tempfile::tempdir().unwrap();
        let active_registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, abandoned_registry.path());
        let private_audio = crashed.work_dir.path().join("Audio").join("take.wav");
        std::fs::create_dir_all(private_audio.parent().unwrap()).unwrap();
        super::super::fixtures::write_tone(&private_audio, 480);
        let source = crashed.project.add_audio_source(
            "Take",
            auris_core::AssetPath::inside("Audio/take.wav"),
            480,
            48_000.0,
            2,
        );
        let track = crashed.project.add_audio_track("Take");
        crashed
            .project
            .add_audio_clip(track, source, auris_core::time::Ticks::ZERO)
            .unwrap();
        crashed.dirty = true;
        crashed.save_autosave().unwrap();
        let abandoned_workspace = crashed.work_dir.path().to_path_buf();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);
        let snapshot = recovery_snapshots_in(abandoned_registry.path())
            .unwrap()
            .pop()
            .unwrap();

        let mut recovered = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut recovered, active_registry.path());
        let missing = recovered.recover_autosave(&snapshot).unwrap();
        let recovered_audio = recovered.project.audio_sources[&source]
            .path
            .resolve(recovered.project_folder())
            .unwrap();

        assert!(missing.is_empty());
        assert!(recovered.path().is_none());
        assert!(recovered.is_dirty());
        assert!(recovered_audio.starts_with(recovered.work_dir.path()));
        assert!(recovered_audio.is_file());
        assert!(!abandoned_workspace.exists());
        assert!(recovered.autosave_path().is_file());
        assert!(
            recovery_snapshots_in(abandoned_registry.path())
                .unwrap()
                .is_empty(),
            "the freshly recovered session still owns its lease"
        );
        assert!(
            recovery_snapshots_in(active_registry.path())
                .unwrap()
                .is_empty()
        );

        drop(recovered.work_dir._lease.take());
        assert_eq!(
            recovery_snapshots_in(abandoned_registry.path())
                .unwrap()
                .len(),
            1,
            "the replacement snapshot becomes discoverable after a crash releases its lease"
        );
    }

    #[test]
    fn recovery_never_reopens_or_overwrites_the_source_document() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let saved_root = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        let saved = crashed
            .save_as(&saved_root.path().join("Source.auris"))
            .unwrap()
            .document;
        let saved_bytes = std::fs::read(&saved).unwrap();
        crashed
            .add_default_instrument_track("Unsaved lead")
            .unwrap();
        crashed.save_autosave().unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);
        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();

        let mut recovered = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        let previous_sound_scope = recovered.sound_scope.clone();
        recovered.recover_autosave(&snapshot).unwrap();

        assert!(recovered.path().is_none());
        assert!(recovered.is_dirty());
        assert_ne!(
            recovered.sound_scope, previous_sound_scope,
            "a recovered unsaved document cannot inherit discovery handles from the replaced one"
        );
        assert_eq!(recovered.project().tracks[0].name, "Unsaved lead");
        assert_eq!(std::fs::read(saved).unwrap(), saved_bytes);
    }

    #[test]
    fn explicit_document_replacement_clears_the_recovery_snapshot() {
        let registry = tempfile::tempdir().unwrap();
        let saved = tempfile::tempdir().unwrap();
        let mut session = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut session, registry.path());
        session.add_default_instrument_track("Lead").unwrap();
        session.save_autosave().unwrap();
        assert!(session.autosave_path().is_file());
        assert!(session.work_dir.path().join(METADATA_FILE).is_file());

        session.save_as(&saved.path().join("Song.auris")).unwrap();

        assert!(!session.autosave_path().exists());
        assert!(!session.work_dir.path().join(METADATA_FILE).exists());
        assert!(recovery_snapshots_in(registry.path()).unwrap().is_empty());
    }

    #[test]
    fn starting_a_new_project_clears_the_recovery_snapshot() {
        let registry = tempfile::tempdir().unwrap();
        let mut session = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut session, registry.path());
        session.add_default_instrument_track("Lead").unwrap();
        session.save_autosave().unwrap();

        session.new_project();

        assert!(!session.autosave_path().exists());
        assert!(!session.work_dir.path().join(METADATA_FILE).exists());
        assert!(recovery_snapshots_in(registry.path()).unwrap().is_empty());
    }

    #[test]
    fn opening_a_project_clears_the_previous_recovery_snapshot() {
        let registry = tempfile::tempdir().unwrap();
        let saved = tempfile::tempdir().unwrap();
        let document = saved.path().join("Opened.auris");
        let mut opened = Project::new("Opened", 48_000.0);
        auris_io::save_project(&document, &mut opened).unwrap();
        let mut session = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut session, registry.path());
        session.add_default_instrument_track("Lead").unwrap();
        session.save_autosave().unwrap();

        session.open(&document).unwrap();

        assert!(!session.autosave_path().exists());
        assert!(!session.work_dir.path().join(METADATA_FILE).exists());
        assert!(recovery_snapshots_in(registry.path()).unwrap().is_empty());
    }

    #[test]
    fn a_recovery_entry_can_be_discarded_without_opening_its_document() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed.add_default_instrument_track("Lead").unwrap();
        crashed.save_autosave().unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);
        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();

        Session::discard_recovery(&snapshot).unwrap();

        assert!(recovery_snapshots_in(registry.path()).unwrap().is_empty());
    }

    #[test]
    fn cancelling_before_discard_commit_keeps_the_recovery_entry() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed.add_default_instrument_track("Lead").unwrap();
        crashed.save_autosave().unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);
        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();

        let discarded = Session::begin_discard_recovery(&snapshot)
            .run(&AtomicBool::new(true))
            .unwrap();

        assert!(discarded.is_none());
        assert!(snapshot.document().is_file());
        assert_eq!(recovery_snapshots_in(registry.path()).unwrap().len(), 1);
        Session::discard_recovery(&snapshot).unwrap();
    }

    #[test]
    fn late_cancel_and_cleanup_failure_still_report_a_committed_discard() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed
            .add_default_instrument_track("Only recovery")
            .unwrap();
        crashed.save_autosave().unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);
        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();
        let cancelled = AtomicBool::new(false);
        let mut quarantined = None;

        let discarded = discard_recovery_with_cleanup(snapshot.clone(), &cancelled, |quarantine| {
            assert!(
                !snapshot.workspace.exists(),
                "rename is the commit boundary"
            );
            assert_eq!(quarantine.parent(), Some(registry.path()));
            quarantined = Some(quarantine.to_path_buf());
            cancelled.store(true, Ordering::Relaxed);
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated cleanup failure",
            ))
        })
        .unwrap();
        let quarantine = quarantined.expect("cleanup received the quarantine path");

        assert_eq!(discarded, Some(snapshot));
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(
            quarantine.is_dir(),
            "failed cleanup leaves quarantine for later"
        );
        assert!(
            recovery_snapshots_in(registry.path()).unwrap().is_empty(),
            "a committed quarantine must never reappear as a recovery offer"
        );
        RecoveryQuarantineCleanupJob {
            roots: vec![registry.path().to_path_buf()],
        }
        .run()
        .unwrap();
        assert!(
            !quarantine.exists(),
            "the next background sweep removes a previously failed quarantine"
        );
    }

    #[test]
    fn quarantine_cleanup_removes_only_prefixed_real_directories() {
        let registry = tempfile::tempdir().unwrap();
        let quarantine = registry.path().join(".discarded-old-session");
        let unrelated = registry.path().join("keep-this-directory");
        let prefixed_file = registry.path().join(".discarded-not-a-directory");
        std::fs::create_dir(&quarantine).unwrap();
        std::fs::write(quarantine.join("payload"), b"discarded").unwrap();
        std::fs::create_dir(&unrelated).unwrap();
        std::fs::write(&prefixed_file, b"not a quarantine directory").unwrap();

        RecoveryQuarantineCleanupJob {
            roots: vec![registry.path().to_path_buf()],
        }
        .run()
        .unwrap();

        assert!(!quarantine.exists());
        assert!(unrelated.is_dir());
        assert!(prefixed_file.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_cleanup_never_deletes_or_follows_a_directory_symlink() {
        use std::os::unix::fs::symlink;

        let registry = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("sentinel");
        std::fs::write(&sentinel, b"keep").unwrap();
        let alias = registry.path().join(".discarded-alias");
        symlink(outside.path(), &alias).unwrap();

        RecoveryQuarantineCleanupJob {
            roots: vec![registry.path().to_path_buf()],
        }
        .run()
        .unwrap();

        assert!(std::fs::symlink_metadata(&alias).is_ok());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");
        std::fs::remove_file(alias).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn quarantine_cleanup_never_deletes_or_follows_a_directory_reparse_point() {
        use std::os::windows::fs::symlink_dir;

        let registry = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("sentinel");
        std::fs::write(&sentinel, b"keep").unwrap();
        let alias = registry.path().join(".discarded-alias");
        if let Err(error) = symlink_dir(outside.path(), &alias) {
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314)
            {
                return;
            }
            panic!("could not create directory symlink: {error}");
        }
        assert!(is_reparse_point(
            &std::fs::symlink_metadata(&alias).unwrap()
        ));

        RecoveryQuarantineCleanupJob {
            roots: vec![registry.path().to_path_buf()],
        }
        .run()
        .unwrap();

        assert!(std::fs::symlink_metadata(&alias).is_ok());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");
        std::fs::remove_dir(alias).unwrap();
    }

    #[test]
    fn damaged_metadata_does_not_hide_an_intact_project_snapshot() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed.add_default_instrument_track("Lead").unwrap();
        crashed.save_autosave().unwrap();
        std::fs::write(crashed.work_dir.path().join(METADATA_FILE), b"not json").unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);

        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();

        assert_eq!(snapshot.project_name(), None);
        assert!(auris_io::load_project(snapshot.document()).is_ok());
    }

    #[test]
    fn oversized_metadata_fields_are_not_returned_to_a_frontend() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed.add_default_instrument_track("Lead").unwrap();
        crashed.save_autosave().unwrap();
        let oversized_source =
            std::env::temp_dir().join("x".repeat(MAX_METADATA_SOURCE_PATH_BYTES));
        let metadata = serde_json::json!({
            "version": METADATA_VERSION,
            "project_name": "x".repeat(MAX_METADATA_PROJECT_NAME_BYTES + 1),
            "source_document": oversized_source,
        });
        std::fs::write(
            crashed.work_dir.path().join(METADATA_FILE),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);

        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();

        assert_eq!(snapshot.project_name(), None);
        assert_eq!(snapshot.source_document(), None);
    }

    #[test]
    fn oversized_metadata_documents_are_not_loaded() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed.add_default_instrument_track("Lead").unwrap();
        crashed.save_autosave().unwrap();
        let metadata = serde_json::json!({
            "version": METADATA_VERSION,
            "project_name": "Safe-sized title",
            "source_document": null,
            "ignored_padding": "x".repeat(MAX_METADATA_BYTES as usize),
        });
        std::fs::write(
            crashed.work_dir.path().join(METADATA_FILE),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);

        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();

        assert_eq!(snapshot.project_name(), None);
        assert_eq!(snapshot.source_document(), None);
    }

    #[test]
    fn an_unrelated_directory_is_not_a_recovery_entry() {
        let registry = tempfile::tempdir().unwrap();
        let unrelated = registry.path().join("somebody-elses-files");
        std::fs::create_dir(&unrelated).unwrap();
        let document = unrelated.join(AUTOSAVE_FILE);
        let mut project = Project::new("Not registered", 48_000.0);
        auris_io::save_project(&document, &mut project).unwrap();

        let snapshots = recovery_snapshots_in(registry.path()).unwrap();

        assert!(snapshots.is_empty());
        assert!(document.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_symlink_is_never_offered_or_deleted() {
        use std::os::unix::fs::symlink;

        let registry = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let document = outside.path().join(AUTOSAVE_FILE);
        let mut project = Project::new("Outside", 48_000.0);
        auris_io::save_project(&document, &mut project).unwrap();
        let alias = registry.path().join("session-alias");
        symlink(outside.path(), &alias).unwrap();

        let snapshots = recovery_snapshots_in(registry.path()).unwrap();

        assert!(snapshots.is_empty());
        assert!(document.is_file());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_workspace_reparse_point_is_never_offered_or_deleted() {
        use std::os::windows::fs::symlink_dir;

        let registry = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let document = outside.path().join(AUTOSAVE_FILE);
        let mut project = Project::new("Outside", 48_000.0);
        auris_io::save_project(&document, &mut project).unwrap();
        let alias = registry.path().join("session-alias");
        if let Err(error) = symlink_dir(outside.path(), &alias) {
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314)
            {
                // Windows requires Developer Mode or SeCreateSymbolicLinkPrivilege. The branch is
                // still compiled on every Windows run; machines able to create a reparse point
                // exercise the filesystem behavior as well.
                return;
            }
            panic!("could not create directory symlink: {error}");
        }
        assert!(is_reparse_point(
            &std::fs::symlink_metadata(&alias).unwrap()
        ));

        let snapshots = recovery_snapshots_in(registry.path()).unwrap();

        assert!(snapshots.is_empty());
        assert!(document.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_discovered_workspace_replaced_by_a_symlink_is_not_deleted() {
        use std::os::unix::fs::symlink;

        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_document = outside.path().join(AUTOSAVE_FILE);
        let mut outside_project = Project::new("Outside", 48_000.0);
        auris_io::save_project(&outside_document, &mut outside_project).unwrap();

        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed.add_default_instrument_track("Recovered").unwrap();
        crashed.save_autosave().unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);
        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();
        let quarantined = registry.path().join("quarantined-workspace");
        std::fs::rename(&snapshot.workspace, &quarantined).unwrap();
        symlink(outside.path(), &snapshot.workspace).unwrap();

        let error = Session::discard_recovery(&snapshot).unwrap_err();

        assert!(matches!(error, SessionError::RecoveryUnavailable(_)));
        assert!(outside_document.is_file());
    }

    #[test]
    fn recovery_refuses_to_replace_current_unsaved_work() {
        let registry = tempfile::tempdir().unwrap();
        let replacement = tempfile::tempdir().unwrap();
        let mut crashed = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        move_workspace_into(&mut crashed, registry.path());
        crashed.add_default_instrument_track("Recovered").unwrap();
        crashed.save_autosave().unwrap();
        leave_workspace_behind(&mut crashed, replacement.path());
        drop(crashed);
        let snapshot = recovery_snapshots_in(registry.path())
            .unwrap()
            .pop()
            .unwrap();
        let mut current = crate::Session::new(crate::SessionOptions::headless()).unwrap();
        current.add_default_instrument_track("Current").unwrap();

        let error = current.recover_autosave(&snapshot).unwrap_err();

        assert!(matches!(error, SessionError::RecoveryWouldDiscardChanges));
        assert_eq!(current.project().tracks[0].name, "Current");
        assert!(snapshot.document().is_file());
    }
}
