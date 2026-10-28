use super::*;
use auris_core::Project as CoreProject;
use auris_core::pipeline::content::ContentLimits;
use auris_core::time::{Ticks, TimeSignature};

fn limits() -> ContentLimits {
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

fn editor() -> NativeDocumentEditor {
    let project = CoreProject::new("timeline", 48_000.0);
    NativeDocumentEditor::new(
        auris_io::project_to_native(&project, limits()).unwrap(),
        limits(),
    )
    .unwrap()
}

#[test]
fn timeline_edit_updates_context_and_undo_restores_it() {
    let mut editor = editor();
    let before = editor.document().context.tempo_map.clone();
    let operation = NativeEdit::Timeline(NativeTimelineEdit::SetTempo {
        at: Ticks::QUARTER,
        bpm: 90.0,
    });
    let receipt = editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::SetTempoPoint,
            &[operation],
        )
        .unwrap();
    assert!(receipt.needs_prepare);
    assert_eq!(editor.document().context.tempo_map.points()[1].bpm, 90.0);
    editor.undo(editor.revision().document).unwrap();
    assert_eq!(editor.document().context.tempo_map, before);
}

#[test]
fn invalid_tempo_is_atomic_and_does_not_create_history() {
    let mut editor = editor();
    let before = editor.document().clone();
    let result = editor.apply(
        0,
        NativeEditOwner::Agent(2),
        crate::Edit::SetTempoPoint,
        &[NativeEdit::Timeline(NativeTimelineEdit::SetTempo {
            at: Ticks::ZERO,
            bpm: f64::NAN,
        })],
    );
    assert!(result.is_err());
    assert_eq!(editor.document(), &before);
    assert!(!editor.can_undo());
    assert_eq!(editor.revision(), NativeEditRevision::default());
}

#[test]
fn extreme_loop_ticks_are_normalized_without_panicking() {
    let mut editor = editor();
    let operation = NativeEdit::Timeline(NativeTimelineEdit::SetLoopRegion {
        start: Ticks(i64::MAX),
        end: Ticks(i64::MIN),
    });
    editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::SetLoopRegion,
            &[operation],
        )
        .unwrap();
    let region = editor.document().context.loop_region.unwrap();
    assert!(region.0 >= Ticks::ZERO);
    assert!(region.1 >= region.0);
}

#[test]
fn grid_and_name_are_presentation_only_receipts() {
    let mut editor = editor();
    let receipt = editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::ExternalChanges,
            &[
                NativeEdit::Timeline(NativeTimelineEdit::SetGrid { grid: Ticks::ZERO }),
                NativeEdit::Timeline(NativeTimelineEdit::SetName {
                    name: "arrangement".into(),
                }),
            ],
        )
        .unwrap();
    assert!(receipt.changed);
    assert!(!receipt.needs_prepare);
    assert_eq!(editor.document().context.grid, Ticks(1));
}

#[test]
fn point_tempo_snaps_to_harmony_grid_and_at_tempo_changes_current_segment() {
    let mut editor = editor();
    editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::SetTempoPoint,
            &[NativeEdit::Timeline(NativeTimelineEdit::SetTempo {
                at: Ticks(500),
                bpm: 90.0,
            })],
        )
        .unwrap();
    assert_eq!(
        editor.document().context.tempo_map.points()[1].tick,
        Ticks::QUARTER
    );

    let revision = editor.revision().document;
    editor
        .apply(
            revision,
            NativeEditOwner::Human,
            crate::Edit::ChangeTempo(Ticks::QUARTER),
            &[NativeEdit::Timeline(NativeTimelineEdit::SetTempoAt {
                at: Ticks(500),
                bpm: 80.0,
            })],
        )
        .unwrap();
    assert_eq!(editor.document().context.tempo_map.initial_bpm(), 80.0);
}

#[test]
fn remove_tempo_and_signature_act_on_change_in_force() {
    let mut editor = editor();
    let bar = editor.document().context.signatures.bar_start(2);
    editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::SetSignaturePoint,
            &[NativeEdit::Timeline(NativeTimelineEdit::SetSignature {
                at: bar,
                signature: TimeSignature::new(3, 4),
            })],
        )
        .unwrap();
    let revision = editor.revision().document;
    editor
        .apply(
            revision,
            NativeEditOwner::Human,
            crate::Edit::RemoveSignaturePoint,
            &[NativeEdit::Timeline(NativeTimelineEdit::RemoveSignature {
                at: bar + Ticks(10),
            })],
        )
        .unwrap();
    assert_eq!(editor.document().context.signatures.points().len(), 1);
}

#[test]
fn extreme_timeline_positions_do_not_overflow_snapping() {
    let mut editor = editor();
    for operation in [
        NativeTimelineEdit::SetTempo {
            at: Ticks(i64::MAX),
            bpm: 90.0,
        },
        NativeTimelineEdit::SetSignature {
            at: Ticks(i64::MAX),
            signature: TimeSignature::new(7, 8),
        },
    ] {
        let revision = editor.revision().document;
        editor
            .apply(
                revision,
                NativeEditOwner::Human,
                crate::Edit::ExternalChanges,
                &[NativeEdit::Timeline(operation)],
            )
            .unwrap();
    }
}

#[test]
fn transaction_keeps_original_authored_snapshot_across_multiple_applies() {
    let mut editor = editor();
    let original = editor.document().clone();
    editor
        .begin_transaction(0, NativeEditOwner::Human, crate::Edit::ExternalChanges)
        .unwrap();
    let revision = editor.revision().document;
    editor
        .apply(
            revision,
            NativeEditOwner::Human,
            crate::Edit::SetTempoPoint,
            &[NativeEdit::Timeline(NativeTimelineEdit::SetTempo {
                at: Ticks::QUARTER,
                bpm: 90.0,
            })],
        )
        .unwrap();
    let revision = editor.revision().document;
    editor
        .apply(
            revision,
            NativeEditOwner::Human,
            crate::Edit::SetSignaturePoint,
            &[NativeEdit::Timeline(NativeTimelineEdit::SetSignature {
                at: Ticks::QUARTER * 4,
                signature: TimeSignature::new(3, 4),
            })],
        )
        .unwrap();
    editor
        .end_transaction(editor.revision().document, NativeEditOwner::Human)
        .unwrap();
    editor.undo(editor.revision().document).unwrap();
    assert_eq!(
        editor.document().context.tempo_map,
        original.context.tempo_map
    );
    assert_eq!(
        editor.document().context.signatures,
        original.context.signatures
    );
}

#[test]
fn mixed_authored_and_unrecorded_batch_keeps_setting_after_undo() {
    let mut editor = editor();
    editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::ExternalChanges,
            &[
                NativeEdit::Timeline(NativeTimelineEdit::SetTempo {
                    at: Ticks::QUARTER,
                    bpm: 90.0,
                }),
                NativeEdit::Timeline(NativeTimelineEdit::SetGrid { grid: Ticks(123) }),
            ],
        )
        .unwrap();
    editor.undo(editor.revision().document).unwrap();
    assert_eq!(editor.document().context.tempo_map.points().len(), 1);
    assert_eq!(editor.document().context.grid, Ticks(123));
}

#[test]
fn pure_unrecorded_change_does_not_consume_redo_branch() {
    let mut editor = editor();
    editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::SetTempoPoint,
            &[NativeEdit::Timeline(NativeTimelineEdit::SetTempo {
                at: Ticks::QUARTER,
                bpm: 90.0,
            })],
        )
        .unwrap();
    editor.undo(editor.revision().document).unwrap();
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::ExternalChanges,
            &[NativeEdit::Timeline(NativeTimelineEdit::SetGrid {
                grid: Ticks(123),
            })],
        )
        .unwrap();
    assert!(editor.can_redo());
    editor.redo(editor.revision().document).unwrap();
    assert_eq!(editor.document().context.grid, Ticks(123));
}

#[test]
fn unrelated_unrecorded_settings_do_not_swallow_loop_region_undo() {
    let mut editor = editor();
    let before = editor.document().context.loop_region;
    editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::SetLoopRegion,
            &[NativeEdit::Timeline(NativeTimelineEdit::SetLoopRegion {
                start: Ticks::QUARTER,
                end: Ticks::QUARTER * 4,
            })],
        )
        .unwrap();
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::ExternalChanges,
            &[NativeEdit::Timeline(NativeTimelineEdit::SetGrid {
                grid: Ticks(123),
            })],
        )
        .unwrap();
    editor.undo(editor.revision().document).unwrap();
    assert_eq!(editor.document().context.loop_region, before);
    assert_eq!(editor.document().context.grid, Ticks(123));
}

#[test]
fn authored_noop_with_unrecorded_change_does_not_add_undo_or_clear_redo() {
    let mut editor = editor();
    let name = editor.document().context.name.clone();
    let tempo = editor.document().context.tempo_map.bpm_at(Ticks::ZERO);
    editor
        .apply(
            0,
            NativeEditOwner::Human,
            crate::Edit::ExternalChanges,
            &[
                NativeEdit::Timeline(NativeTimelineEdit::SetTempoAt {
                    at: Ticks::ZERO,
                    bpm: tempo,
                }),
                NativeEdit::Timeline(NativeTimelineEdit::SetName {
                    name: format!("{name} edited"),
                }),
            ],
        )
        .unwrap();
    assert!(!editor.can_undo());
    assert!(editor.is_dirty());
}
