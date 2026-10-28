use super::*;
use auris_core::pipeline::content::{ProcessorKind, SourceContent};
use auris_core::project::{ClipId, StableMidiClip, StageId};
use auris_core::{Note, NoteTransform, Project, Ticks, TrackId};

pub(crate) fn limits() -> ContentLimits {
    ContentLimits {
        sources: 64,
        clips: 128,
        stages: 512,
        processors: 512,
        groups: 128,
        tracks: 128,
        edges: 1024,
        external_ids: 128,
        automations: 128,
        automation_points: 1024,
        bindings: 128,
    }
}

fn fixture() -> (NativeDocumentEditor, TrackId, ClipId) {
    let mut project = Project::new("native history", 48_000.0);
    let track = project.add_instrument_track("Keys", "unavailable.test.instrument");
    let clip = project
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    let target = project.midi_clip_mut(clip).unwrap();
    target
        .notes
        .push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
    target.transforms = vec![
        NoteTransform::Gate { amount: 0.8 },
        NoteTransform::Transpose { semitones: 12 },
    ];
    let native = auris_io::project_to_native(&project, limits()).unwrap();
    (
        NativeDocumentEditor::new(native, limits()).unwrap(),
        track,
        clip,
    )
}

fn notes(editor: &NativeDocumentEditor, id: ClipId) -> &StableMidiClip {
    editor
        .document()
        .content
        .sources
        .iter()
        .find_map(|source| match &source.content {
            SourceContent::Notes(clips) => clips.iter().find(|clip| clip.id == id),
            _ => None,
        })
        .unwrap()
}

fn apply(editor: &mut NativeDocumentEditor, operations: &[NativeEdit]) -> NativeEditReceipt {
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            operations,
        )
        .unwrap()
}

#[test]
fn a_failed_batch_preserves_document_history_and_revision() {
    let (mut editor, track, _) = fixture();
    let before = editor.document().clone();
    let result = editor.apply(
        0,
        NativeEditOwner::Agent(7),
        Edit::ExternalChanges,
        &[
            NativeEdit::RenameTrack {
                track,
                name: "Must not land".into(),
            },
            NativeEdit::SetTrackHeight {
                track,
                height: f32::NAN,
            },
        ],
    );
    assert!(result.is_err());
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.revision(), NativeEditRevision::default());
    assert!(!editor.can_undo());
    assert!(!editor.is_dirty());
}

#[test]
fn oversized_payload_is_rejected_before_cloning_or_recording_an_edit() {
    let (mut editor, track, _) = fixture();
    let before = editor.document().clone();
    let result = editor.apply(
        0,
        NativeEditOwner::Agent(3),
        Edit::RenameTrack,
        &[NativeEdit::RenameTrack {
            track,
            name: "x".repeat(MAX_NATIVE_EDIT_BYTES + 1),
        }],
    );
    assert!(matches!(
        result,
        Err(NativeEditError::Content(ContentError::Limit(
            "command_bytes"
        )))
    ));
    assert_eq!(editor.document(), &before);
    assert!(!editor.can_undo());
    assert_eq!(editor.revision().document, 0);
}

#[test]
fn stage_ids_survive_reorder_undo_redo_and_reopen() {
    let (mut editor, _, clip) = fixture();
    let stages = notes(&editor, clip).transforms.clone();
    let receipt = apply(
        &mut editor,
        &[NativeEdit::MoveStage {
            clip,
            stage: stages[0].id,
            offset: 1,
        }],
    );
    assert!(receipt.needs_prepare);
    assert_eq!(
        notes(&editor, clip).transforms,
        vec![stages[1].clone(), stages[0].clone()]
    );
    editor.undo(editor.revision().document).unwrap();
    assert_eq!(notes(&editor, clip).transforms, stages);
    assert!(!editor.is_dirty());
    editor.redo(editor.revision().document).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Native.auris");
    auris_io::save_native(&path, editor.document(), limits()).unwrap();
    let reopened = auris_io::load_native(&path, limits()).unwrap();
    assert_eq!(reopened, *editor.document());
    editor.acknowledge_save(editor.revision().document).unwrap();
    assert!(!editor.is_dirty());
}

#[test]
fn undo_does_not_reuse_allocated_stage_ids() {
    let (mut editor, _, clip) = fixture();
    apply(
        &mut editor,
        &[NativeEdit::AppendStage {
            clip,
            transform: NoteTransform::Gate { amount: 0.2 },
        }],
    );
    let first = notes(&editor, clip).transforms.last().unwrap().id;
    let high_water = editor.document().content.next_id;
    editor.undo(editor.revision().document).unwrap();
    assert_eq!(editor.document().content.next_id, high_water);
    assert!(!editor.is_dirty());
    apply(
        &mut editor,
        &[NativeEdit::AppendStage {
            clip,
            transform: NoteTransform::Gate { amount: 0.3 },
        }],
    );
    assert!(notes(&editor, clip).transforms.last().unwrap().id.0 > first.0);
    assert!(!editor.can_redo());
}

#[test]
fn agent_requests_cannot_edit_or_finish_a_human_gesture() {
    let (mut editor, track, _) = fixture();
    editor
        .begin_transaction(0, NativeEditOwner::Human, Edit::RenameTrack)
        .unwrap();
    let operation = NativeEdit::RenameTrack {
        track,
        name: "Human".into(),
    };
    for owner in [NativeEditOwner::Agent(1), NativeEditOwner::Agent(2)] {
        assert!(matches!(
            editor.apply(
                0,
                owner,
                Edit::ExternalChanges,
                std::slice::from_ref(&operation)
            ),
            Err(NativeEditError::TransactionActive)
        ));
        assert!(matches!(
            editor.end_transaction(0, owner),
            Err(NativeEditError::TransactionActive)
        ));
    }
    apply(&mut editor, &[operation]);
    assert!(!editor.can_undo());
    assert!(matches!(
        editor.undo(1),
        Err(NativeEditError::TransactionActive)
    ));
    assert!(editor.end_transaction(1, NativeEditOwner::Human).unwrap());
    editor.undo(1).unwrap();
    assert_eq!(editor.document().track_presentation[0].name, "Keys");
}

#[test]
fn reverting_a_gesture_preserves_the_existing_redo_branch() {
    let (mut editor, track, _) = fixture();
    apply(
        &mut editor,
        &[NativeEdit::RenameTrack {
            track,
            name: "Original edit".into(),
        }],
    );
    editor.undo(1).unwrap();
    editor
        .begin_transaction(2, NativeEditOwner::Human, Edit::RenameTrack)
        .unwrap();
    apply(
        &mut editor,
        &[NativeEdit::RenameTrack {
            track,
            name: "Abandoned".into(),
        }],
    );
    editor
        .revert_transaction(3, NativeEditOwner::Human)
        .unwrap();
    assert!(editor.can_redo());
    editor.redo(4).unwrap();
    assert_eq!(
        editor.document().track_presentation[0].name,
        "Original edit"
    );
}

#[test]
fn presentation_changes_do_not_advance_the_musical_revision() {
    let (mut editor, track, _) = fixture();
    let receipt = apply(
        &mut editor,
        &[
            NativeEdit::RenameTrack {
                track,
                name: "New name".into(),
            },
            NativeEdit::SetTrackHeight {
                track,
                height: 96.0,
            },
        ],
    );
    assert_eq!(
        receipt.revision,
        NativeEditRevision {
            document: 1,
            musical: 0
        }
    );
    assert!(!receipt.needs_prepare);
    let strip = editor.document().content.groups[0].strip;
    let receipt = apply(
        &mut editor,
        &[NativeEdit::SetStrip {
            node: strip,
            gain_db: Some(-3.0),
            pan: None,
            mute: None,
            solo: None,
        }],
    );
    assert_eq!(
        receipt.revision,
        NativeEditRevision {
            document: 2,
            musical: 1
        }
    );
    assert!(receipt.needs_prepare);
}

#[test]
fn noops_and_stale_requests_preserve_redo_and_revision() {
    let (mut editor, track, _) = fixture();
    apply(
        &mut editor,
        &[NativeEdit::RenameTrack {
            track,
            name: "Changed".into(),
        }],
    );
    editor.undo(1).unwrap();
    let receipt = apply(
        &mut editor,
        &[NativeEdit::RenameTrack {
            track,
            name: "Keys".into(),
        }],
    );
    assert!(!receipt.changed);
    assert_eq!(receipt.revision.document, 2);
    assert!(matches!(
        editor.redo(0),
        Err(NativeEditError::StaleRevision { .. })
    ));
    assert!(matches!(
        editor.acknowledge_save(0),
        Err(NativeEditError::StaleRevision { .. })
    ));
    assert!(editor.can_redo());
}

#[test]
fn clip_replacement_uses_the_existing_bounded_note_contract() {
    let (mut editor, _, clip) = fixture();
    let before = editor.document().clone();
    let invalid = Note::new(64, Ticks::QUARTER * 4, Ticks::QUARTER);
    assert!(
        editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[NativeEdit::ReplaceClipNotes {
                    clip,
                    notes: vec![invalid]
                }]
            )
            .is_err()
    );
    assert_eq!(editor.document(), &before);
    let original_stages = notes(&editor, clip).transforms.clone();
    apply(
        &mut editor,
        &[NativeEdit::ReplaceClipNotes {
            clip,
            notes: vec![Note::new(65, Ticks::ZERO, Ticks::QUARTER)],
        }],
    );
    assert_eq!(notes(&editor, clip).notes[0].pitch, 65);
    assert_eq!(notes(&editor, clip).transforms, original_stages);
}

#[test]
fn shared_source_is_edited_once_without_copying_consumers() {
    let (mut editor, track, clip) = fixture();
    let group = editor
        .document()
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .unwrap()
        .group;
    apply(&mut editor, &[NativeEdit::DuplicateBranch { group }]);
    let source_count = editor.document().content.sources.len();
    let processors = editor.document().content.processors.clone();
    apply(
        &mut editor,
        &[NativeEdit::ReplaceClipNotes {
            clip,
            notes: vec![Note::new(67, Ticks::ZERO, Ticks::QUARTER)],
        }],
    );
    assert_eq!(editor.document().content.sources.len(), source_count);
    assert_eq!(editor.document().content.processors, processors);
    assert_eq!(notes(&editor, clip).notes[0].pitch, 67);
    assert_eq!(
        editor
            .document()
            .content
            .processors
            .iter()
            .filter(|processor| matches!(processor.kind, ProcessorKind::Source(_)))
            .count(),
        1
    );
}

#[test]
fn missing_stages_nonfinite_controls_and_revision_exhaustion_are_atomic() {
    let (mut editor, _, clip) = fixture();
    let before = editor.document().clone();
    assert!(
        editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[NativeEdit::RemoveStage {
                    clip,
                    stage: StageId(u64::MAX)
                }]
            )
            .is_err()
    );
    assert!(
        editor
            .apply(
                0,
                NativeEditOwner::Human,
                Edit::ExternalChanges,
                &[NativeEdit::AppendStage {
                    clip,
                    transform: NoteTransform::Gate { amount: f32::NAN }
                }]
            )
            .is_err()
    );
    assert_eq!(editor.document(), &before);
    editor.revision.document = u64::MAX;
    assert!(matches!(
        editor.apply(
            u64::MAX,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::RemoveStage {
                clip,
                stage: notes(&editor, clip).transforms[0].id
            }]
        ),
        Err(NativeEditError::RevisionExhausted)
    ));
    assert_eq!(editor.document(), &before);
    assert!(!editor.can_undo());
}
