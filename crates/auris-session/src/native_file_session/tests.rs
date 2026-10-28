use super::*;
use crate::native_edit::{NativeEdit, NativeEditOwner};
use auris_core::pipeline::content::ContentLimits;
use auris_core::{Project, Ticks};
use tempfile::TempDir;

fn limits() -> ContentLimits {
    crate::native_edit::tests::limits()
}

fn fixture() -> NativeDocument {
    let mut project = Project::new("native persistence", 48_000.0);
    let track = project.add_instrument_track("Keys", "unavailable.instrument");
    project
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER)
        .unwrap();
    auris_io::project_to_native(&project, limits()).unwrap()
}

fn save_new(session: &mut NativeFileSession, chosen: &std::path::Path) {
    let job = session.begin_save_as(chosen).unwrap();
    session.finish_save(job.run().unwrap()).unwrap();
}

#[test]
fn native_save_reopen_after_edit_preserves_current_document() {
    let temp = TempDir::new().unwrap();
    let chosen = temp.path().join("Song.auris");
    let mut session = NativeFileSession::new(fixture(), limits()).unwrap();
    save_new(&mut session, &chosen);
    let track = session.editor().document().track_presentation[0].id;
    session
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::RenameTrack,
            &[NativeEdit::RenameTrack {
                track,
                name: "Edited".into(),
            }],
        )
        .unwrap();
    let job = session.begin_save().unwrap();
    session.finish_save(job.run().unwrap()).unwrap();
    let reopened = NativeFileSession::open(session.path().unwrap(), limits()).unwrap();
    assert_eq!(
        reopened.editor().document().track_presentation[0].name,
        "Edited"
    );
}

#[test]
fn edited_legacy_upgrade_writes_current_document_and_exact_backup() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("Legacy.auris");
    let mut project = Project::new("legacy", 48_000.0);
    project.add_instrument_track("Original", "unavailable.instrument");
    auris_io::save_project(&path, &mut project).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut session = NativeFileSession::open(&path, limits()).unwrap();
    let track = session.editor().document().track_presentation[0].id;
    session
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::RenameTrack,
            &[NativeEdit::RenameTrack {
                track,
                name: "Edited legacy".into(),
            }],
        )
        .unwrap();
    let job = session.begin_save().unwrap();
    session.finish_save(job.run().unwrap()).unwrap();
    assert_eq!(
        std::fs::read(path.with_file_name("Legacy.auris.legacy-backup")).unwrap(),
        original
    );
    assert_eq!(
        auris_io::load_native(&path, limits()).unwrap(),
        *session.editor().document()
    );
}

#[test]
fn stale_save_cannot_overwrite_newer_edit() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("Song.auris");
    let mut session = NativeFileSession::new(fixture(), limits()).unwrap();
    save_new(&mut session, &path);
    let job = session.begin_save().unwrap();
    let track = session.editor().document().track_presentation[0].id;
    session
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::RenameTrack,
            &[NativeEdit::RenameTrack {
                track,
                name: "Newer".into(),
            }],
        )
        .unwrap();
    assert!(matches!(
        session.finish_save(job.run().unwrap()),
        Err(NativeFileSessionError::StaleSave)
    ));
}

#[test]
fn external_file_change_before_begin_save_is_rejected() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("Song.auris");
    let mut session = NativeFileSession::new(fixture(), limits()).unwrap();
    save_new(&mut session, &path);
    std::fs::write(session.path().unwrap(), b"external").unwrap();
    let job = session.begin_save().unwrap();
    assert!(matches!(
        session.finish_save(job.run().unwrap()),
        Err(NativeFileSessionError::Io(
            NativeFileError::DestinationChanged(_)
        ))
    ));
}

#[test]
fn save_result_from_another_session_is_rejected_even_with_shared_document() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("Song.auris");
    let mut first = NativeFileSession::new(fixture(), limits()).unwrap();
    save_new(&mut first, &path);
    let mut second = NativeFileSession::open(first.path().unwrap(), limits()).unwrap();
    let job = first.begin_save().unwrap();
    assert!(matches!(
        second.finish_save(job.run().unwrap()),
        Err(NativeFileSessionError::StaleSave)
    ));
}

#[test]
fn checkpoint_restore_is_one_undo_step_and_keeps_allocator_high_water() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("Song.auris");
    let mut session = NativeFileSession::new(fixture(), limits()).unwrap();
    save_new(&mut session, &path);
    let checkpoint = session.create_checkpoint("before").unwrap();
    let old_next = session.editor().document().content.next_id;
    let track = session.editor().document().track_presentation[0].id;
    session
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::RenameTrack,
            &[NativeEdit::RenameTrack {
                track,
                name: "Changed".into(),
            }],
        )
        .unwrap();
    session
        .restore_checkpoint(&checkpoint, 1, NativeEditOwner::Human)
        .unwrap();
    assert!(session.editor().can_undo());
    assert!(session.editor().document().content.next_id >= old_next);
    session.editor_mut().undo(2).unwrap();
    assert_eq!(
        session.editor().document().track_presentation[0].name,
        "Changed"
    );
}

#[test]
fn failed_stale_save_as_never_copies_assets_or_changes_the_saved_path() {
    let temp = TempDir::new().unwrap();
    let mut project = Project::new("recorded", 48_000.0);
    let track = project.add_audio_track("Audio");
    let first_folder = temp.path().join("First");
    std::fs::create_dir_all(first_folder.join("Audio")).unwrap();
    std::fs::write(first_folder.join("Audio/take.wav"), b"fixture asset bytes").unwrap();
    project.add_audio_source("Take", AssetPath::inside("Audio/take.wav"), 32, 48_000.0, 1);
    let first_path = first_folder.join("First.auris");
    auris_io::save_native(
        &first_path,
        &auris_io::project_to_native(&project, limits()).unwrap(),
        limits(),
    )
    .unwrap();
    let mut session = NativeFileSession::open(&first_path, limits()).unwrap();
    let result = session
        .begin_save_as(&temp.path().join("Second.auris"))
        .unwrap()
        .run()
        .unwrap();
    session
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::RenameTrack,
            &[NativeEdit::RenameTrack {
                track,
                name: "Changed".into(),
            }],
        )
        .unwrap();
    assert!(matches!(
        session.finish_save(result),
        Err(NativeFileSessionError::StaleSave)
    ));
    assert!(!temp.path().join("Second/Audio/take.wav").exists());
    assert!(!temp.path().join("Second/Second.auris").exists());
    assert_eq!(session.path(), Some(first_path.as_path()));
    assert!(session.editor().is_dirty());
}

#[test]
fn a_deleted_saved_document_is_not_recreated_by_a_stale_in_place_save() {
    let temp = TempDir::new().unwrap();
    let mut session = NativeFileSession::new(fixture(), limits()).unwrap();
    save_new(&mut session, &temp.path().join("Song.auris"));
    let path = session.path().unwrap().to_path_buf();
    let result = session.begin_save().unwrap().run().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(session.finish_save(result).is_err());
    assert!(!path.exists());
}

#[test]
fn save_as_refuses_another_document_in_the_same_project_folder() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("Song");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Other.auris"), b"existing project").unwrap();
    let mut session = NativeFileSession::new(fixture(), limits()).unwrap();
    let result = session
        .begin_save_as(&folder.join("Song.auris"))
        .unwrap()
        .run()
        .unwrap();
    assert!(session.finish_save(result).is_err());
    assert_eq!(
        std::fs::read(folder.join("Other.auris")).unwrap(),
        b"existing project"
    );
    assert!(!folder.join("Song.auris").exists());
}
