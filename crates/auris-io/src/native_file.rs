//! Native pipeline document persistence.
//!
//! Native files use the same human-readable JSON and atomic sibling publication as legacy
//! projects, but carry a separate envelope version.  Keeping the version at the envelope makes
//! the legacy reader reject a native file before it attempts to decode the native payload.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use auris_core::pipeline::content::{ContentLimits, ProcessorKind, SourceContent};
use auris_core::pipeline::document::NativeDocument;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::error::IoError;
use crate::project_file::{
    MAX_PROJECT_FILE_BYTES, new_staged_file, publish_ready_staged_file, read_project_source,
};

/// Errors specific to native pipeline persistence.
#[derive(Debug, Error)]
pub enum NativeFileError {
    /// The underlying bounded file operation failed.
    #[error(transparent)]
    Io(#[from] IoError),
    /// JSON encoding or decoding failed.
    #[error("native pipeline JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// The native envelope uses an unsupported schema version.
    #[error(
        "native pipeline format version {found} does not match the supported version {supported}"
    )]
    VersionMismatch {
        /// Version recorded in the file.
        found: u32,
        /// Version understood by this build.
        supported: u32,
    },
    /// The native document failed ownership validation.
    #[error("invalid native pipeline document: {0}")]
    Validation(String),
    /// The serialised native envelope exceeds the bounded document size.
    #[error(
        "native pipeline file is too large to write: {observed} bytes; the limit is {limit} bytes"
    )]
    TooLarge {
        /// Lower bound on the encoded byte count when the writer stopped.
        observed: u64,
        /// Maximum accepted document size.
        limit: u64,
    },
    /// The captured legacy source changed during the upgrade transaction.
    #[error("legacy source changed while preparing native upgrade: {0}")]
    SourceChanged(PathBuf),
    /// The destination no longer contains the bytes observed before staging.
    #[error("native destination changed while staging: {0}")]
    DestinationChanged(PathBuf),
    /// Another Auris writer currently owns the project's cross-process write lock.
    #[error("project is already being written: {0}")]
    Locked(PathBuf),
    /// A backup must not name the source or the destination being replaced.
    #[error("legacy backup must be separate from source and destination: {0}")]
    BackupAliasesDocument(PathBuf),
    /// Exact-byte legacy backup could not be created.
    #[error(transparent)]
    Backup(#[from] crate::ProjectBackupError),
}

/// Result returned by native persistence APIs.
pub type NativeResult<T> = std::result::Result<T, NativeFileError>;

/// Native pipeline envelope schema version.
pub const NATIVE_FORMAT_VERSION: u32 = 33;

/// A native pipeline file, including its independent schema marker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeEnvelope {
    /// Native envelope schema version.
    pub format_version: u32,
    /// The document owned by the native pipeline model.
    pub document: NativeDocument,
}

/// A serialised native document awaiting atomic publication.
pub struct StagedNativeDocument {
    file: NamedTempFile,
    path: PathBuf,
}

impl StagedNativeDocument {
    /// Atomically replaces the destination with the staged native document.
    fn publish(self) -> NativeResult<()> {
        publish_ready_staged_file(self.file, &self.path).map_err(NativeFileError::Io)
    }

    /// Atomically claims an absent destination, refusing to replace an existing project.
    fn publish_noclobber(self) -> NativeResult<()> {
        crate::project_file::publish_ready_staged_file_noclobber(self.file, &self.path)
            .map_err(NativeFileError::Io)
    }
}

/// Serialises and synchronises a native document into a private sibling of `path`.
pub fn stage_native(
    path: &Path,
    document: &NativeDocument,
    limits: ContentLimits,
) -> NativeResult<StagedNativeDocument> {
    validate_native_document(document, limits)?;
    #[derive(Serialize)]
    struct EnvelopeRef<'a> {
        format_version: u32,
        document: &'a NativeDocument,
    }
    let envelope = EnvelopeRef {
        format_version: NATIVE_FORMAT_VERSION,
        document,
    };
    let mut output = BoundedDocument {
        bytes: Vec::new(),
        limit: MAX_PROJECT_FILE_BYTES,
        exceeded: false,
    };
    if let Err(error) = serde_json::to_writer_pretty(&mut output, &envelope) {
        return Err(if output.exceeded {
            NativeFileError::TooLarge {
                observed: MAX_PROJECT_FILE_BYTES as u64 + 1,
                limit: MAX_PROJECT_FILE_BYTES as u64,
            }
        } else {
            NativeFileError::Json(error)
        });
    }
    let encoded: NativeEnvelope = serde_json::from_slice(&output.bytes)?;
    validate_native_document(&encoded.document, limits)?;
    if encoded.document != *document {
        return Err(NativeFileError::Validation(
            "native document cannot round-trip without losing values".into(),
        ));
    }
    let mut file = new_staged_file(path)?;
    file.write_all(&output.bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| IoError::from_fs(path, error))?;
    Ok(StagedNativeDocument {
        file,
        path: path.to_path_buf(),
    })
}

struct BoundedDocument {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for BoundedDocument {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other(
                "native document exceeds its file byte limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Writes a native pipeline document to an absent destination.
///
/// This intentionally refuses to overwrite an existing path. Updating an existing project must
/// use a caller-owned transaction that proves its current format and, for legacy upgrades,
/// preserves the captured source bytes first.
pub fn save_native(
    path: &Path,
    document: &NativeDocument,
    limits: ContentLimits,
) -> NativeResult<()> {
    let _lock = acquire_write_lock(path)?;
    stage_native(path, document, limits)?.publish_noclobber()
}

/// Publishes a staged native update only when the destination still has the observed bytes.
pub fn publish_native_update(
    staged: StagedNativeDocument,
    expected_current: &[u8],
    limits: ContentLimits,
) -> NativeResult<()> {
    let _lock = acquire_write_lock(&staged.path)?;
    let destination = staged.path.clone();
    let current = read_project_source(&destination, MAX_PROJECT_FILE_BYTES)?;
    if current != expected_current {
        return Err(NativeFileError::DestinationChanged(destination));
    }
    let _ = decode_native_bytes(expected_current, limits)?;
    staged.publish()
}

/// Converts a captured legacy source and publishes a native destination safely.
///
/// The source is checked before and after staging. Exact source bytes are preserved at `backup`
/// before publication. In-place upgrades replace the source only after backup succeeds;
/// another destination must be absent. The project lock serializes cooperating Auris writers;
/// when source and destination are in different project folders, both folder locks are held.
pub fn publish_native_upgrade(
    candidate: &crate::NativeMigrationCandidate,
    source: &Path,
    destination: &Path,
    backup: &Path,
    limits: ContentLimits,
) -> NativeResult<()> {
    publish_upgrade(candidate, source, destination, backup, limits, || {})
}

fn publish_upgrade(
    candidate: &crate::NativeMigrationCandidate,
    source: &Path,
    destination: &Path,
    backup: &Path,
    limits: ContentLimits,
    after_stage: impl FnOnce(),
) -> NativeResult<()> {
    let _locks = acquire_write_locks(source, destination)?;
    let backup_identity = path_identity(backup);
    if backup_identity == path_identity(source) || backup_identity == path_identity(destination) {
        return Err(NativeFileError::BackupAliasesDocument(backup.to_path_buf()));
    }
    if !candidate
        .source_matches(source)
        .map_err(NativeFileError::Io)?
    {
        return Err(NativeFileError::SourceChanged(source.to_path_buf()));
    }
    candidate.preserve_backup(backup)?;
    let staged = stage_native(destination, candidate.document(), limits)?;
    after_stage();
    if !candidate
        .source_matches(source)
        .map_err(NativeFileError::Io)?
    {
        return Err(NativeFileError::SourceChanged(source.to_path_buf()));
    }
    if destination == source {
        staged.publish()
    } else {
        staged.publish_noclobber()
    }
}

fn path_identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn acquire_write_lock(path: &Path) -> NativeResult<File> {
    let folder = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    acquire_write_lock_folder(folder)
}

fn acquire_write_locks(first: &Path, second: &Path) -> NativeResult<Vec<File>> {
    let mut folders = [first, second]
        .into_iter()
        .map(|path| {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
        })
        .map(|folder| std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf()))
        .collect::<Vec<_>>();
    folders.sort();
    folders.dedup();
    folders
        .iter()
        .map(|folder| acquire_write_lock_folder(folder))
        .collect()
}

fn acquire_write_lock_folder(folder: &Path) -> NativeResult<File> {
    let lock_path = folder.join(".auris-write.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| NativeFileError::Io(IoError::from_fs(&lock_path, error)))?;
    file.try_lock()
        .map_err(|_| NativeFileError::Locked(lock_path))?;
    Ok(file)
}

/// Loads and validates one native pipeline document with the bounded project-file reader.
pub fn load_native(path: &Path, limits: ContentLimits) -> NativeResult<NativeDocument> {
    let bytes = read_project_source(path, MAX_PROJECT_FILE_BYTES)?;
    decode_native_bytes(&bytes, limits)
}

fn decode_native_bytes(bytes: &[u8], limits: ContentLimits) -> NativeResult<NativeDocument> {
    #[derive(Deserialize)]
    struct VersionProbe {
        format_version: u32,
    }
    let probe: VersionProbe = serde_json::from_slice(bytes)?;
    if probe.format_version != NATIVE_FORMAT_VERSION {
        return Err(NativeFileError::VersionMismatch {
            found: probe.format_version,
            supported: NATIVE_FORMAT_VERSION,
        });
    }
    let envelope: NativeEnvelope = serde_json::from_slice(bytes)?;
    validate_native_document(&envelope.document, limits)?;
    Ok(envelope.document)
}

fn validate_native_document(document: &NativeDocument, limits: ContentLimits) -> NativeResult<()> {
    document
        .validate_ownership(limits)
        .map_err(|error| NativeFileError::Validation(error.to_string()))?;
    if !document.context.sample_rate.is_finite() || document.context.sample_rate <= 0.0 {
        return Err(NativeFileError::Validation(
            "sample rate must be finite and positive".to_owned(),
        ));
    }
    for processor in &document.content.processors {
        match &processor.kind {
            ProcessorKind::Strip(state) => {
                if !state.gain_db.is_finite()
                    || !state.pan.is_finite()
                    || state.target_lufs.is_some_and(|value| !value.is_finite())
                {
                    return Err(NativeFileError::Validation(
                        "strip state contains a non-finite value".to_owned(),
                    ));
                }
            }
            ProcessorKind::Send { level_db, .. } if !level_db.is_finite() => {
                return Err(NativeFileError::Validation(
                    "send level contains a non-finite value".to_owned(),
                ));
            }
            ProcessorKind::Singer(singer)
                if !singer.frame_hop.is_finite() || singer.frame_hop <= 0.0 =>
            {
                return Err(NativeFileError::Validation(
                    "singer frame hop must be finite and positive".to_owned(),
                ));
            }
            _ => {}
        }
    }
    for source in &document.content.sources {
        if let SourceContent::Notes(clips) = &source.content {
            for clip in clips {
                clip.performer_clip()
                    .looped_note_instances()
                    .map_err(|error| {
                        NativeFileError::Validation(format!(
                            "clip {:?} exceeds note loop budget: {error}",
                            clip.id
                        ))
                    })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::Project;
    use auris_core::project::TrackKind;

    // Each test project owns its directory and write lock, including under parallel Cargo tests.
    struct TempFile {
        _directory: tempfile::TempDir,
        path: PathBuf,
    }

    impl TempFile {
        fn new(name: &str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(name);
            Self {
                _directory: directory,
                path,
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    fn limits() -> ContentLimits {
        ContentLimits {
            sources: 10_000,
            clips: 100_000,
            stages: 100_000,
            processors: 10_000,
            groups: 10_000,
            tracks: 10_000,
            edges: 100_000,
            external_ids: 10_000,
            automations: 10_000,
            automation_points: 100_000,
            bindings: 100_000,
        }
    }

    #[test]
    fn native_save_reopens_with_stable_topology_ids() {
        let mut source = Project::new("stable", 48_000.0);
        let track = source.add_instrument_track("Lead", "auris.synth.chiptune");
        source
            .add_midi_clip(
                track,
                "phrase",
                auris_core::Ticks::ZERO,
                auris_core::Ticks::from_beats(4.0),
            )
            .unwrap();
        if let Some(TrackKind::Instrument(instrument)) =
            source.track_mut(track).map(|track| &mut track.kind)
        {
            instrument.instrument_state.set_hosted_bytes(&[1, 2, 3, 4]);
            instrument
                .instrument_state
                .params
                .insert("cutoff".into(), 0.25);
            instrument.clips[0].notes.push(auris_core::Note::new(
                64,
                auris_core::Ticks::ZERO,
                auris_core::Ticks::QUARTER,
            ));
            instrument.clips[0].transforms = vec![
                auris_core::NoteTransform::Gate { amount: 0.7 },
                auris_core::NoteTransform::Humanize {
                    amount: 0.2,
                    seed: 42,
                },
            ];
        }
        source.automation.set_point(
            auris_core::ParamTarget::TrackGain(track),
            None,
            auris_core::automation::AutomationCurve::Linear,
            auris_core::Ticks::ZERO,
            -3.0,
        );
        let mut document = crate::project_to_native(&source, limits()).unwrap();
        let group = document.content.tracks[0].group;
        document
            .content
            .duplicate_branch(group, &document.external_ids(), limits())
            .unwrap();
        let file = TempFile::new("native-round-trip.auris");
        save_native(file.path(), &document, limits()).unwrap();
        let loaded = load_native(file.path(), limits()).unwrap();
        assert_eq!(loaded, document);
        assert_eq!(loaded.content.processors, document.content.processors);
    }

    #[test]
    fn legacy_loader_rejects_native_envelope_before_payload_decode() {
        let source = Project::new("legacy-reject", 48_000.0);
        let document = crate::project_to_native(&source, limits()).unwrap();
        let file = TempFile::new("native-reject.auris");
        save_native(file.path(), &document, limits()).unwrap();
        assert!(matches!(
            crate::load_project(file.path()),
            Err(crate::error::IoError::ProjectVersionMismatch { .. })
        ));
    }

    #[test]
    fn staged_native_save_leaves_destination_until_publish() {
        let source = Project::new("staged", 48_000.0);
        let document = crate::project_to_native(&source, limits()).unwrap();
        let file = TempFile::new("native-staged.auris");
        let staged = stage_native(file.path(), &document, limits()).unwrap();
        assert!(!file.path().exists());
        staged.publish().unwrap();
        assert_eq!(load_native(file.path(), limits()).unwrap(), document);
    }

    #[test]
    fn upgrade_requires_unchanged_source_and_preserves_exact_backup() {
        let mut source_project = Project::new("upgrade", 48_000.0);
        let source = TempFile::new("legacy-source.auris");
        crate::save_project(source.path(), &mut source_project).unwrap();
        let candidate = crate::load_native_candidate(source.path(), limits()).unwrap();
        let destination = TempFile::new("native-target.auris");
        let backup = TempFile::new("legacy-backup.auris");
        let destination_path = destination.path().to_path_buf();
        let backup_path = backup.path().to_path_buf();
        crate::publish_native_upgrade(
            &candidate,
            source.path(),
            &destination_path,
            &backup_path,
            limits(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&backup_path).unwrap(),
            candidate.source_bytes()
        );
        assert_eq!(
            load_native(&destination_path, limits()).unwrap(),
            *candidate.document()
        );

        std::fs::write(source.path(), b"changed after migration").unwrap();
        let second_target = TempFile::new("native-stale-target.auris");
        let second_backup = TempFile::new("legacy-stale-backup.auris");
        assert!(matches!(
            crate::publish_native_upgrade(
                &candidate,
                source.path(),
                second_target.path(),
                second_backup.path(),
                limits(),
            ),
            Err(NativeFileError::SourceChanged(_))
        ));
        assert!(!second_target.path().exists());
        assert!(!second_backup.path().exists());
    }

    #[test]
    fn native_save_refuses_existing_destination() {
        let source = Project::new("collision", 48_000.0);
        let document = crate::project_to_native(&source, limits()).unwrap();
        let file = TempFile::new("native-collision.auris");
        std::fs::write(file.path(), b"sentinel").unwrap();
        assert!(matches!(
            save_native(file.path(), &document, limits()),
            Err(NativeFileError::Io(IoError::ExportDestinationExists(_)))
        ));
        assert_eq!(std::fs::read(file.path()).unwrap(), b"sentinel");
    }

    #[test]
    fn native_update_rejects_legacy_expected_bytes_without_writing() {
        let mut legacy = Project::new("legacy-current", 48_000.0);
        let file = TempFile::new("native-update-legacy.auris");
        crate::save_project(file.path(), &mut legacy).unwrap();
        let expected = std::fs::read(file.path()).unwrap();
        let document = crate::project_to_native(&legacy, limits()).unwrap();
        let staged = stage_native(file.path(), &document, limits()).unwrap();
        assert!(matches!(
            publish_native_update(staged, &expected, limits()),
            Err(NativeFileError::VersionMismatch { .. })
        ));
        assert_eq!(std::fs::read(file.path()).unwrap(), expected);
    }

    #[test]
    fn future_native_version_is_rejected_before_malformed_payload() {
        let file = TempFile::new("native-future-version.auris");
        std::fs::write(
            file.path(),
            br#"{"format_version":999,"document":{"not":"a document"}}"#,
        )
        .unwrap();
        assert!(matches!(
            load_native(file.path(), limits()),
            Err(NativeFileError::VersionMismatch { found: 999, .. })
        ));
    }

    #[test]
    fn native_save_rejects_nonfinite_context_and_strip_values() {
        let source = Project::new("nonfinite", 48_000.0);
        let mut document = crate::project_to_native(&source, limits()).unwrap();
        let file = TempFile::new("native-nonfinite.auris");
        document.context.sample_rate = f64::NAN;
        assert!(matches!(
            save_native(file.path(), &document, limits()),
            Err(NativeFileError::Validation(_))
        ));
        document.context.sample_rate = 48_000.0;
        if let ProcessorKind::Strip(state) = &mut document.content.processors[0].kind {
            state.gain_db = f32::INFINITY;
        }
        assert!(matches!(
            save_native(file.path(), &document, limits()),
            Err(NativeFileError::Validation(_))
        ));
        assert!(!file.path().exists());
    }

    #[test]
    fn native_save_rejects_excessive_note_loop_expansion() {
        let mut source = Project::new("loop-budget", 48_000.0);
        let track = source.add_instrument_track("Lead", "auris.synth.chiptune");
        source
            .add_midi_clip(
                track,
                "phrase",
                auris_core::Ticks::ZERO,
                auris_core::Ticks::QUARTER,
            )
            .unwrap();
        let mut document = crate::project_to_native(&source, limits()).unwrap();
        if let Some(SourceContent::Notes(clips)) =
            document.content.sources.first_mut().map(|s| &mut s.content)
        {
            clips[0].loop_end = auris_core::Ticks::from_beats(1_000_000.0);
        }
        let file = TempFile::new("native-loop-budget.auris");
        assert!(matches!(
            save_native(file.path(), &document, limits()),
            Err(NativeFileError::Validation(_))
        ));
        assert!(!file.path().exists());
    }

    #[test]
    fn in_place_upgrade_keeps_exact_legacy_bytes_and_native_identities() {
        let source = TempFile::new("upgrade-in-place.auris");
        let backup = source.path().with_extension("auris.legacy-backup");
        let mut project = Project::new("original", 48_000.0);
        crate::save_project(source.path(), &mut project).unwrap();
        let mut bytes = std::fs::read(source.path()).unwrap();
        bytes.extend_from_slice(b"\n  \n");
        std::fs::write(source.path(), &bytes).unwrap();
        let candidate = crate::load_native_candidate(source.path(), limits()).unwrap();
        publish_native_upgrade(&candidate, source.path(), source.path(), &backup, limits())
            .unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), bytes);
        assert_eq!(
            load_native(source.path(), limits()).unwrap(),
            *candidate.document()
        );
        assert!(matches!(
            crate::load_project(source.path()),
            Err(IoError::ProjectVersionMismatch {
                found: NATIVE_FORMAT_VERSION,
                ..
            })
        ));
    }

    #[test]
    fn conflicting_or_aliasing_backup_leaves_legacy_source_unchanged() {
        let source = TempFile::new("backup-conflict.auris");
        let backup = source.path().with_extension("backup");
        let mut project = Project::new("original", 48_000.0);
        crate::save_project(source.path(), &mut project).unwrap();
        let candidate = crate::load_native_candidate(source.path(), limits()).unwrap();
        std::fs::write(&backup, b"another original").unwrap();
        assert!(matches!(
            publish_native_upgrade(&candidate, source.path(), source.path(), &backup, limits()),
            Err(NativeFileError::Backup(_))
        ));
        assert_eq!(
            std::fs::read(source.path()).unwrap(),
            candidate.source_bytes()
        );
        assert_eq!(std::fs::read(&backup).unwrap(), b"another original");
        assert!(matches!(
            publish_native_upgrade(
                &candidate,
                source.path(),
                source.path(),
                source.path(),
                limits()
            ),
            Err(NativeFileError::BackupAliasesDocument(_))
        ));
        assert_eq!(
            std::fs::read(source.path()).unwrap(),
            candidate.source_bytes()
        );
    }

    #[test]
    fn changed_source_after_staging_is_not_overwritten() {
        let source = TempFile::new("changed-during-stage.auris");
        let backup = source.path().with_extension("backup");
        let mut project = Project::new("captured", 48_000.0);
        crate::save_project(source.path(), &mut project).unwrap();
        let candidate = crate::load_native_candidate(source.path(), limits()).unwrap();
        let result = publish_upgrade(
            &candidate,
            source.path(),
            source.path(),
            &backup,
            limits(),
            || {
                project.name = "external writer".into();
                crate::save_project(source.path(), &mut project).unwrap();
            },
        );
        assert!(matches!(result, Err(NativeFileError::SourceChanged(_))));
        assert_eq!(
            crate::load_project(source.path()).unwrap().name,
            "external writer"
        );
        assert_eq!(std::fs::read(&backup).unwrap(), candidate.source_bytes());
        assert_eq!(
            std::fs::read_dir(source.path().parent().unwrap())
                .unwrap()
                .count(),
            3
        );
    }

    #[test]
    fn native_update_refuses_stale_preimage_and_reopens_a_successful_update() {
        let source = TempFile::new("native-update.auris");
        let mut document =
            crate::project_to_native(&Project::new("first", 48_000.0), limits()).unwrap();
        save_native(source.path(), &document, limits()).unwrap();
        let original = std::fs::read(source.path()).unwrap();
        document.context.name = "second".into();
        let second = stage_native(source.path(), &document, limits()).unwrap();
        let stale = stage_native(source.path(), &document, limits()).unwrap();
        publish_native_update(second, &original, limits()).unwrap();
        assert_eq!(load_native(source.path(), limits()).unwrap(), document);
        let current = std::fs::read(source.path()).unwrap();
        assert!(matches!(
            publish_native_update(stale, &original, limits()),
            Err(NativeFileError::DestinationChanged(_))
        ));
        assert_eq!(std::fs::read(source.path()).unwrap(), current);
    }

    #[test]
    fn competing_auris_writer_prevents_new_native_publication() {
        let source = TempFile::new("lock-contention.auris");
        let document =
            crate::project_to_native(&Project::new("locked", 48_000.0), limits()).unwrap();
        let lock = acquire_write_lock(source.path()).unwrap();
        assert!(matches!(
            save_native(source.path(), &document, limits()),
            Err(NativeFileError::Locked(_))
        ));
        assert!(!source.path().exists());
        drop(lock);
        save_native(source.path(), &document, limits()).unwrap();
        assert_eq!(load_native(source.path(), limits()).unwrap(), document);
    }

    #[test]
    fn discarding_a_staged_native_document_preserves_the_destination() {
        let source = TempFile::new("discard-stage.auris");
        let document =
            crate::project_to_native(&Project::new("discard", 48_000.0), limits()).unwrap();
        std::fs::write(source.path(), b"original").unwrap();
        let staged = stage_native(source.path(), &document, limits()).unwrap();
        assert_eq!(std::fs::read(source.path()).unwrap(), b"original");
        drop(staged);
        assert_eq!(std::fs::read(source.path()).unwrap(), b"original");
        assert_eq!(
            std::fs::read_dir(source.path().parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn upgrade_locks_both_source_and_destination_folders() {
        let mut legacy = Project::new("cross-folder", 48_000.0);
        let source = TempFile::new("cross-source.auris");
        crate::save_project(source.path(), &mut legacy).unwrap();
        let candidate = crate::load_native_candidate(source.path(), limits()).unwrap();
        let destination = TempFile::new("cross-destination.auris");
        let backup = TempFile::new("cross-backup.auris");
        let source_bytes = std::fs::read(source.path()).unwrap();
        let source_lock = acquire_write_lock(source.path()).unwrap();
        assert!(matches!(
            publish_native_upgrade(
                &candidate,
                source.path(),
                destination.path(),
                backup.path(),
                limits(),
            ),
            Err(NativeFileError::Locked(_))
        ));
        assert_eq!(std::fs::read(source.path()).unwrap(), source_bytes);
        assert!(!destination.path().exists());
        assert!(!backup.path().exists());
        drop(source_lock);
        publish_native_upgrade(
            &candidate,
            source.path(),
            destination.path(),
            backup.path(),
            limits(),
        )
        .unwrap();
        assert_eq!(std::fs::read(backup.path()).unwrap(), source_bytes);
        assert_eq!(
            load_native(destination.path(), limits()).unwrap(),
            *candidate.document()
        );
    }
}
