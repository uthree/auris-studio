use std::path::Path;
use std::sync::{Arc, Barrier};
use std::thread;

use auris_io::{
    LegacyBackupOutcome, MAX_PROJECT_FILE_BYTES, ProjectBackupError, preserve_legacy_project_backup,
};

fn destination(directory: &Path) -> std::path::PathBuf {
    directory.join("legacy.auris.bak")
}

#[test]
fn backup_preserves_exact_legacy_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"{  \"format_version\": 31,\n\t\"unknown\": true }\n";
    let path = destination(directory.path());

    assert_eq!(
        preserve_legacy_project_backup(bytes, &path).unwrap(),
        LegacyBackupOutcome::Created
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn identical_existing_backup_is_idempotent_but_different_bytes_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = destination(directory.path());
    let bytes = b"legacy bytes\n";
    std::fs::write(&path, bytes).unwrap();

    assert_eq!(
        preserve_legacy_project_backup(bytes, &path).unwrap(),
        LegacyBackupOutcome::AlreadyPresent
    );
    assert!(matches!(
        preserve_legacy_project_backup(b"different", &path),
        Err(ProjectBackupError::ExistingBytesDiffer { .. })
    ));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn directory_at_destination_is_never_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let path = destination(directory.path());
    std::fs::create_dir(&path).unwrap();

    assert!(matches!(
        preserve_legacy_project_backup(b"bytes", &path),
        Err(ProjectBackupError::ExistingDirectory { .. })
    ));
    assert!(path.is_dir());
}

#[test]
fn oversized_bytes_are_rejected_before_staging() {
    let directory = tempfile::tempdir().unwrap();
    let path = destination(directory.path());
    let bytes = vec![b'x'; MAX_PROJECT_FILE_BYTES + 1];

    assert!(matches!(
        preserve_legacy_project_backup(&bytes, &path),
        Err(ProjectBackupError::TooLarge { .. })
    ));
    assert!(!path.exists());
}

#[test]
fn concurrent_same_byte_backups_have_one_creator_and_no_clobber() {
    let directory = tempfile::tempdir().unwrap();
    let path = Arc::new(destination(directory.path()));
    let barrier = Arc::new(Barrier::new(2));
    let bytes: &'static [u8] = b"same captured legacy bytes";
    let mut handles = Vec::new();
    for _ in 0..2 {
        let path = Arc::clone(&path);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            preserve_legacy_project_backup(bytes, &path).unwrap()
        }));
    }
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == LegacyBackupOutcome::Created)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == LegacyBackupOutcome::AlreadyPresent)
            .count(),
        1
    );
    assert_eq!(std::fs::read(path.as_ref()).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn symlink_at_destination_is_not_accepted_as_a_backup() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let path = destination(directory.path());
    std::fs::write(&target, b"same bytes").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();

    assert!(matches!(
        preserve_legacy_project_backup(b"same bytes", &path),
        Err(ProjectBackupError::ExistingSymlink { .. })
    ));
}
