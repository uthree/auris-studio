use super::*;
use crate::native_edit::{NativeEdit, NativeTrackEdit, NativeTrackKindSpec};
use crate::{Edit, SessionOptions};

fn fixture() -> (Session, NativeSessionTransition, auris_core::TrackId) {
    let mut source = Session::new(SessionOptions {
        shipped_fonts: false,
        shipped_dictionary: false,
        ..SessionOptions::headless()
    })
    .unwrap();
    let track = source.add_bus_track("Original");
    let transition = source
        .capture_native_transition(crate::native_edit::tests::limits())
        .unwrap();
    (source, transition, track)
}

fn rename(track: auris_core::TrackId, name: &str) -> NativeEdit {
    NativeEdit::RenameTrack {
        track,
        name: name.into(),
    }
}

#[test]
fn native_session_gesture_has_one_history_step_and_preserves_ids_through_redo() {
    let (source, mut transition, track) = fixture();
    let original = source.project().clone();
    source
        .begin_native_transition_transaction(
            &mut transition,
            0,
            NativeEditOwner::Human,
            Edit::RenameTrack,
        )
        .unwrap();
    source
        .apply_native_transition(
            &mut transition,
            0,
            NativeEditOwner::Human,
            Edit::RenameTrack,
            &[rename(track, "First")],
        )
        .unwrap();
    let receipt = source
        .apply_native_transition(
            &mut transition,
            1,
            NativeEditOwner::Human,
            Edit::RenameTrack,
            &[
                rename(track, "Final"),
                NativeEdit::Track(NativeTrackEdit::Add {
                    name: "Aux".into(),
                    kind: NativeTrackKindSpec::Bus,
                }),
            ],
        )
        .unwrap();
    assert!(!receipt.created.is_empty());
    assert!(matches!(
        source.undo_native_transition(&mut transition, 2),
        Err(NativeTransitionError::Edit(
            NativeEditError::TransactionActive
        ))
    ));
    assert!(
        source
            .end_native_transition_transaction(&mut transition, 2, NativeEditOwner::Human,)
            .unwrap()
    );
    let edited = transition.editor().document().clone();
    source
        .undo_native_transition(&mut transition, 2)
        .unwrap()
        .unwrap();
    assert_eq!(transition.editor().document().track_presentation.len(), 1);
    assert_eq!(
        transition.editor().document().track_presentation[0].name,
        "Original"
    );
    assert!(
        !transition.editor().can_undo(),
        "both edits belong to the same gesture"
    );
    source
        .redo_native_transition(&mut transition, 3)
        .unwrap()
        .unwrap();
    assert_eq!(transition.editor().document(), &edited);
    assert_eq!(source.project(), &original);
}

#[test]
fn native_session_gesture_rejects_other_owners_and_cancel_retains_redo() {
    let (source, mut transition, track) = fixture();
    source
        .apply_native_transition(
            &mut transition,
            0,
            NativeEditOwner::Human,
            Edit::RenameTrack,
            &[rename(track, "Committed")],
        )
        .unwrap();
    source.undo_native_transition(&mut transition, 1).unwrap();
    source
        .begin_native_transition_transaction(
            &mut transition,
            2,
            NativeEditOwner::Agent(7),
            Edit::RenameTrack,
        )
        .unwrap();
    for owner in [NativeEditOwner::Human, NativeEditOwner::Agent(8)] {
        let rejected = [
            source
                .apply_native_transition(
                    &mut transition,
                    2,
                    owner,
                    Edit::RenameTrack,
                    &[rename(track, "Foreign")],
                )
                .map(|_| ()),
            source
                .end_native_transition_transaction(&mut transition, 2, owner)
                .map(|_| ()),
            source
                .revert_native_transition_transaction(&mut transition, 2, owner)
                .map(|_| ()),
        ];
        for result in rejected {
            assert!(matches!(
                result,
                Err(NativeTransitionError::Edit(
                    NativeEditError::TransactionActive
                ))
            ));
        }
    }
    assert!(matches!(
        source.begin_native_transition_transaction(
            &mut transition,
            2,
            NativeEditOwner::Agent(7),
            Edit::RenameTrack
        ),
        Err(NativeTransitionError::Edit(
            NativeEditError::TransactionActive
        ))
    ));
    source
        .apply_native_transition(
            &mut transition,
            2,
            NativeEditOwner::Agent(7),
            Edit::RenameTrack,
            &[rename(track, "Cancelled")],
        )
        .unwrap();
    source
        .revert_native_transition_transaction(&mut transition, 3, NativeEditOwner::Agent(7))
        .unwrap();
    assert!(transition.editor().can_redo());
    source
        .redo_native_transition(&mut transition, 4)
        .unwrap()
        .unwrap();
    assert_eq!(
        transition.editor().document().track_presentation[0].name,
        "Committed"
    );
}

/// Every entry point must fail before touching either history or a live transaction.
fn rejected_commands(
    source: &Session,
    transition: &mut NativeSessionTransition,
    revision: u64,
    accepts: fn(&NativeTransitionError) -> bool,
) {
    let document = transition.editor().document().clone();
    let before = (
        transition.editor().revision(),
        transition.editor().transaction_owner(),
        transition.editor().can_undo(),
        transition.editor().can_redo(),
    );
    let results = [
        source
            .apply_native_transition(
                transition,
                revision,
                NativeEditOwner::Human,
                Edit::RenameTrack,
                &[],
            )
            .map(|_| ()),
        source.begin_native_transition_transaction(
            transition,
            revision,
            NativeEditOwner::Human,
            Edit::RenameTrack,
        ),
        source
            .end_native_transition_transaction(transition, revision, NativeEditOwner::Human)
            .map(|_| ()),
        source
            .revert_native_transition_transaction(transition, revision, NativeEditOwner::Human)
            .map(|_| ()),
        source
            .undo_native_transition(transition, revision)
            .map(|_| ()),
        source
            .redo_native_transition(transition, revision)
            .map(|_| ()),
    ];
    for result in results {
        let error = result.unwrap_err();
        assert!(accepts(&error), "{error:?}");
    }
    assert_eq!(transition.editor().document(), &document);
    assert_eq!(
        (
            transition.editor().revision(),
            transition.editor().transaction_owner(),
            transition.editor().can_undo(),
            transition.editor().can_redo()
        ),
        before
    );
}

#[test]
fn native_session_commands_reject_foreign_stale_and_busy_origins_atomically() {
    let (mut source, mut transition, _) = fixture();
    let (foreign, _, _) = fixture();
    rejected_commands(&foreign, &mut transition, 0, |e| {
        matches!(e, NativeTransitionError::Stale)
    });
    rejected_commands(&source, &mut transition, 1, |e| {
        matches!(
            e,
            NativeTransitionError::Edit(NativeEditError::StaleRevision { .. })
        )
    });
    source.begin_transaction(Edit::RenameTrack);
    rejected_commands(&source, &mut transition, 0, |e| {
        matches!(e, NativeTransitionError::ActiveEdit)
    });
    source.end_transaction();
    source
        .begin_native_transition_transaction(
            &mut transition,
            0,
            NativeEditOwner::Human,
            Edit::RenameTrack,
        )
        .unwrap();
    source.add_bus_track("Changed origin");
    rejected_commands(&source, &mut transition, 0, |e| {
        matches!(e, NativeTransitionError::Stale)
    });
    // Also reject content changes that bypass the ordinary revision counter (host snapshots).
    let (mut source, mut transition, _) = fixture();
    let revision = source.revision();
    source.project.name = "Different contents".into();
    assert_eq!(source.revision(), revision);
    rejected_commands(&source, &mut transition, 0, |e| {
        matches!(e, NativeTransitionError::Stale)
    });
}

#[test]
fn native_session_batch_failure_leaves_the_previous_history_available() {
    let (source, mut transition, track) = fixture();
    source
        .apply_native_transition(
            &mut transition,
            0,
            NativeEditOwner::Human,
            Edit::RenameTrack,
            &[rename(track, "Kept")],
        )
        .unwrap();
    let before = transition.editor().document().clone();
    assert!(
        source
            .apply_native_transition(
                &mut transition,
                1,
                NativeEditOwner::Human,
                Edit::RenameTrack,
                &[
                    rename(track, "Partial"),
                    rename(auris_core::TrackId(u64::MAX), "Missing")
                ]
            )
            .is_err()
    );
    assert_eq!(transition.editor().document(), &before);
    assert_eq!(transition.editor().revision().document, 1);
    source
        .undo_native_transition(&mut transition, 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        transition.editor().document().track_presentation[0].name,
        "Original"
    );
}
