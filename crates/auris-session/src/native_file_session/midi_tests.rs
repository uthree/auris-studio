use super::*;
use crate::native_edit::{NativeEdit, NativeEditOwner};
use auris_core::pipeline::content::ContentLimits;
use auris_core::pipeline::document::NativeDocument;
use auris_core::{Note, Project, Ticks};
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

fn limits() -> ContentLimits {
    crate::native_edit::tests::limits()
}

fn fixture() -> NativeDocument {
    let mut project = Project::new("MIDI import", 48_000.0);
    let track = project.add_instrument_track("Existing", "unavailable.instrument");
    let clip = project
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER)
        .unwrap();
    project.midi_clip_mut(clip).unwrap().notes = vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)];
    auris_io::project_to_native(&project, limits()).unwrap()
}

fn midi_file(directory: &TempDir) -> std::path::PathBuf {
    let mut project = Project::new("Imported", 48_000.0);
    let track = project.add_instrument_track("Imported keys", "unavailable.instrument");
    let clip = project
        .add_midi_clip(track, "Imported keys", Ticks::ZERO, Ticks::QUARTER)
        .unwrap();
    project.midi_clip_mut(clip).unwrap().notes = vec![Note::new(64, Ticks::ZERO, Ticks::QUARTER)];
    let path = directory.path().join("input.mid");
    std::fs::write(&path, auris_io::write_midi_bytes(&project).unwrap()).unwrap();
    path
}

fn session() -> NativeFileSession {
    NativeFileSession::new(fixture(), limits()).unwrap()
}

#[test]
fn cancelled_worker_does_not_read_or_mutate() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("missing.mid");
    let session = session();
    let job = session
        .begin_midi_append(
            &path,
            Ticks::ZERO,
            NativeEditOwner::Human,
            "fallback.instrument",
            "drums.instrument",
        )
        .unwrap();
    let cancelled = AtomicBool::new(true);
    assert!(job.run(&cancelled).unwrap().is_none());
}

#[test]
fn malformed_worker_returns_error_without_touching_document() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("bad.mid");
    std::fs::write(&path, b"not a midi file").unwrap();
    let session = session();
    let before = session.editor().document().clone();
    let job = session
        .begin_midi_append(
            &path,
            Ticks::ZERO,
            NativeEditOwner::Human,
            "fallback.instrument",
            "drums.instrument",
        )
        .unwrap();
    assert!(job.run(&AtomicBool::new(false)).is_err());
    assert_eq!(session.editor().document(), &before);
}

#[test]
fn stale_and_foreign_results_leave_both_documents_unchanged() {
    let directory = TempDir::new().unwrap();
    let path = midi_file(&directory);
    let mut first = session();
    let job = first
        .begin_midi_append(
            &path,
            Ticks::ZERO,
            NativeEditOwner::Human,
            "fallback.instrument",
            "drums.instrument",
        )
        .unwrap();
    let track = first.editor().document().track_presentation[0].id;
    first
        .editor_mut()
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::ExternalChanges,
            &[NativeEdit::RenameTrack {
                track,
                name: "Changed".into(),
            }],
        )
        .unwrap();
    let result = job.run(&AtomicBool::new(false)).unwrap().unwrap();
    assert!(matches!(
        first.finish_midi_append(result),
        Err(NativeFileSessionError::StaleMidiImport)
    ));

    let source = session();
    let result = source
        .begin_midi_append(
            &path,
            Ticks::ZERO,
            NativeEditOwner::Human,
            "fallback.instrument",
            "drums.instrument",
        )
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap()
        .unwrap();
    let mut foreign = session();
    let before = foreign.editor().document().clone();
    assert!(matches!(
        foreign.finish_midi_append(result),
        Err(NativeFileSessionError::StaleMidiImport)
    ));
    assert_eq!(foreign.editor().document(), &before);
}

#[test]
fn active_gesture_is_rejected_at_begin_and_finish() {
    let directory = TempDir::new().unwrap();
    let path = midi_file(&directory);
    let mut session = session();
    session
        .editor_mut()
        .begin_transaction(0, NativeEditOwner::Human, crate::Edit::ExternalChanges)
        .unwrap();
    assert!(matches!(
        session.begin_midi_append(
            &path,
            Ticks::ZERO,
            NativeEditOwner::Human,
            "fallback.instrument",
            "drums.instrument",
        ),
        Err(NativeFileSessionError::ActiveEdit)
    ));
    session
        .editor_mut()
        .revert_transaction(0, NativeEditOwner::Human)
        .unwrap();

    let job = session
        .begin_midi_append(
            &path,
            Ticks::ZERO,
            NativeEditOwner::Human,
            "fallback.instrument",
            "drums.instrument",
        )
        .unwrap();
    let result = job.run(&AtomicBool::new(false)).unwrap().unwrap();
    session
        .editor_mut()
        .begin_transaction(0, NativeEditOwner::Human, crate::Edit::ExternalChanges)
        .unwrap();
    assert!(matches!(
        session.finish_midi_append(result),
        Err(NativeFileSessionError::ActiveEdit)
    ));
    session
        .editor_mut()
        .revert_transaction(0, NativeEditOwner::Human)
        .unwrap();
}

#[test]
fn valid_append_is_one_undo_step_and_survives_save_reopen() {
    let directory = TempDir::new().unwrap();
    let path = midi_file(&directory);
    let mut session = session();
    let result = session
        .begin_midi_append(
            &path,
            Ticks::QUARTER,
            NativeEditOwner::Human,
            "fallback.instrument",
            "drums.instrument",
        )
        .unwrap()
        .run(&AtomicBool::new(false))
        .unwrap()
        .unwrap();
    let receipt = session.finish_midi_append(result).unwrap();
    assert_eq!(receipt.report.tracks, 1);
    assert_eq!(receipt.report.notes, 1);
    assert_eq!(session.editor().document().content.tracks.len(), 2);
    assert!(session.editor().can_undo());
    let revision = receipt.edit.revision.document;
    session.editor_mut().undo(revision).unwrap();
    assert_eq!(session.editor().document().content.tracks.len(), 1);
    let revision = session.editor().revision().document;
    session.editor_mut().redo(revision).unwrap();
    assert_eq!(session.editor().document().content.tracks.len(), 2);

    let destination = directory.path().join("Saved.auris");
    let save = session.begin_save_as(&destination).unwrap().run().unwrap();
    session.finish_save(save).unwrap();
    let reopened = NativeFileSession::open(session.path().unwrap(), limits()).unwrap();
    assert_eq!(reopened.editor().document(), session.editor().document());
}
