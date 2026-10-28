use super::super::history::Edit;
use super::super::native_edit::{
    NativeCreated, NativeDocumentEditor, NativeEdit, NativeEditError, NativeEditOwner,
};
use crate::midi_import::PreparedMidiTrack;
use auris_core::pipeline::content::{ContentLimits, PluginInstance, SourceContent};
use auris_core::project::{CurvePoint, Note, Project, Scoop};
use auris_core::{PluginState, Ticks};
use std::collections::BTreeMap;

fn limits() -> ContentLimits {
    super::tests::limits()
}

fn editor() -> NativeDocumentEditor {
    let mut project = Project::new("midi import", 48_000.0);
    project.add_instrument_track("Existing", "existing.instrument");
    NativeDocumentEditor::new(
        auris_io::project_to_native(&project, limits()).unwrap(),
        limits(),
    )
    .unwrap()
}

fn track(name: &str) -> PreparedMidiTrack {
    PreparedMidiTrack {
        name: name.into(),
        instrument: PluginInstance {
            plugin_id: "import.instrument".into(),
            state: PluginState::default(),
            file: None,
        },
        drum: false,
        start: Ticks::ZERO,
        length: Ticks::from_beats(2.0),
        notes: vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)],
        bend: vec![CurvePoint {
            at: Ticks::ZERO,
            value: 0.0,
        }],
        controllers: BTreeMap::new(),
    }
}

#[test]
fn import_appends_complete_clip_as_one_edit() {
    let mut editor = editor();
    let original = editor.document().clone();
    let before_sources = editor.document().content.sources.len();
    let receipt = editor
        .apply(
            0,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::ImportMidi {
                tracks: vec![
                    track("Imported"),
                    PreparedMidiTrack {
                        drum: true,
                        name: "Drums".into(),
                        ..track("Drums")
                    },
                ],
                at: Ticks::from_beats(4.0),
            }],
        )
        .unwrap();
    assert!(receipt.changed);
    assert_eq!(
        receipt
            .created
            .iter()
            .filter(|id| matches!(id, NativeCreated::Track(_)))
            .count(),
        2
    );
    assert_eq!(editor.document().content.sources.len(), before_sources + 2);
    let imported = editor
        .document()
        .content
        .sources
        .iter()
        .find_map(|source| match &source.content {
            SourceContent::Notes(clips) => clips.iter().find(|clip| clip.name == "Imported"),
            SourceContent::Audio(_) => None,
        })
        .unwrap();
    assert_eq!(imported.start, Ticks::from_beats(4.0));
    assert_eq!(imported.notes[0].pitch, 60);
    assert_eq!(imported.bend.len(), 1);
    editor.undo(editor.revision().document).unwrap().unwrap();
    assert_eq!(editor.document().content.sources, original.content.sources);
    assert!(editor.document().content.next_id >= original.content.next_id);
}

#[test]
fn import_rejects_invalid_payload_atomically() {
    let mut editor = editor();
    let before = editor.document().clone();
    let result = editor.apply(
        0,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[NativeEdit::ImportMidi {
            tracks: vec![PreparedMidiTrack {
                name: "".into(),
                ..track("bad")
            }],
            at: Ticks::ZERO,
        }],
    );
    assert!(matches!(result, Err(NativeEditError::Invalid(_))));
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.revision().document, 0);
    assert!(!editor.can_undo());
}

#[test]
fn import_rejects_stale_revision_without_mutation() {
    let mut editor = editor();
    let before = editor.document().clone();
    let result = editor.apply(
        9,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[NativeEdit::ImportMidi {
            tracks: vec![track("stale")],
            at: Ticks::ZERO,
        }],
    );
    assert!(matches!(result, Err(NativeEditError::StaleRevision { .. })));
    assert_eq!(editor.document(), &before);
}

#[test]
fn import_rejects_foreign_owner_without_mutation() {
    let mut editor = editor();
    editor
        .begin_transaction(0, NativeEditOwner::Human, Edit::ExternalChanges)
        .unwrap();
    let before = editor.document().clone();
    let result = editor.apply(
        0,
        NativeEditOwner::Agent(7),
        Edit::ExternalChanges,
        &[NativeEdit::ImportMidi {
            tracks: vec![track("foreign")],
            at: Ticks::ZERO,
        }],
    );
    assert!(matches!(result, Err(NativeEditError::TransactionActive)));
    assert_eq!(editor.document(), &before);
    editor
        .revert_transaction(0, NativeEditOwner::Human)
        .unwrap();
}

#[test]
fn import_rejects_bad_notes_and_controller_values_atomically() {
    let mut editor = editor();
    let before = editor.document().clone();
    let mut bad = track("bad");
    bad.notes[0].velocity = f32::NAN;
    bad.controllers.insert(
        1,
        vec![auris_core::project::CurvePoint {
            at: Ticks::ZERO,
            value: 2.0,
        }],
    );
    let result = editor.apply(
        0,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[NativeEdit::ImportMidi {
            tracks: vec![bad],
            at: Ticks::ZERO,
        }],
    );
    assert!(matches!(result, Err(NativeEditError::Invalid(_))));
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.revision().document, 0);
}

#[test]
fn import_rejects_each_untrusted_note_shape() {
    type MutateTrack = fn(&mut PreparedMidiTrack);
    let cases: &[(&str, MutateTrack)] = &[
        ("pitch", |track| track.notes[0].pitch = 128),
        ("phoneme", |track| {
            track.notes[0].phoneme_seconds = vec![f64::NAN]
        }),
        ("scoop", |track| {
            track.notes[0].scoop = Some(Scoop {
                depth: f32::NAN,
                seconds: 0.1,
            })
        }),
        ("controller", |track| {
            track.controllers.insert(
                1,
                vec![CurvePoint {
                    at: Ticks::ZERO,
                    value: 2.0,
                }],
            );
        }),
        ("controller_number", |track| {
            track.controllers.insert(128, Vec::new());
        }),
        ("bend_nan", |track| track.bend[0].value = f32::NAN),
        ("negative_start", |track| track.start = Ticks(-1)),
        ("zero_length", |track| track.length = Ticks::ZERO),
        ("clip_end", |track| track.start = Ticks(i64::MAX)),
        ("note_end", |track| track.notes[0].start = Ticks(i64::MAX)),
        ("curve_end", |track| {
            track.bend[0].at = track.length + Ticks(1)
        }),
    ];
    for (name, mutate) in cases {
        let mut editor = editor();
        let before = editor.document().clone();
        let mut prepared = track(name);
        mutate(&mut prepared);
        let result = editor.apply(
            0,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::ImportMidi {
                tracks: vec![prepared],
                at: Ticks::ZERO,
            }],
        );
        assert!(result.is_err(), "case {name} was accepted");
        assert_eq!(
            editor.document(),
            &before,
            "case {name} mutated the document"
        );
    }
}

#[test]
fn import_rejects_each_exhausted_ownership_limit_atomically() {
    let document = editor().document().clone();
    for resource in [
        "tracks",
        "sources",
        "clips",
        "groups",
        "processors",
        "edges",
    ] {
        let mut bounds = limits();
        match resource {
            "tracks" => bounds.tracks = document.content.tracks.len(),
            "sources" => bounds.sources = document.content.sources.len(),
            "clips" => bounds.clips = 0,
            "groups" => bounds.groups = document.content.groups.len(),
            "processors" => bounds.processors = document.content.processors.len(),
            "edges" => bounds.edges = document.content.edges.len(),
            _ => unreachable!(),
        }
        let mut editor = NativeDocumentEditor::new(document.clone(), bounds).unwrap();
        let result = editor.apply(
            0,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::ImportMidi {
                tracks: vec![track("import")],
                at: Ticks::ZERO,
            }],
        );
        assert!(result.is_err(), "exhausted {resource} was accepted");
        assert_eq!(
            editor.document(),
            &document,
            "changed after exhausting {resource}"
        );
        assert_eq!(editor.revision().document, 0);
        assert!(!editor.can_undo());
    }
}

#[test]
fn import_offset_must_keep_the_complete_clip_inside_the_timeline() {
    for offset in [Ticks(-1), Ticks(i64::MAX - 1)] {
        let mut editor = editor();
        let before = editor.document().clone();
        assert!(
            editor
                .apply(
                    0,
                    NativeEditOwner::Human,
                    Edit::ExternalChanges,
                    &[NativeEdit::ImportMidi {
                        tracks: vec![track("offset")],
                        at: offset
                    }],
                )
                .is_err()
        );
        assert_eq!(editor.document(), &before);
        assert!(!editor.can_undo());
    }
}

#[test]
fn failed_second_import_operation_rolls_back_the_first() {
    let mut editor = editor();
    let before = editor.document().clone();
    let result = editor.apply(
        0,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[
            NativeEdit::ImportMidi {
                tracks: vec![track("first")],
                at: Ticks::ZERO,
            },
            NativeEdit::ImportMidi {
                tracks: vec![PreparedMidiTrack {
                    name: "".into(),
                    ..track("second")
                }],
                at: Ticks::ZERO,
            },
        ],
    );
    assert!(result.is_err());
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.revision().document, 0);
    assert!(!editor.can_undo());
}

#[test]
fn undo_redo_and_reimport_keep_allocated_ids_disjoint() {
    let mut editor = editor();
    let first = editor
        .apply(
            0,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::ImportMidi {
                tracks: vec![track("first")],
                at: Ticks::ZERO,
            }],
        )
        .unwrap();
    let imported = editor.document().clone();
    let highwater = imported.content.next_id;
    editor.undo(editor.revision().document).unwrap().unwrap();
    editor.redo(editor.revision().document).unwrap().unwrap();
    assert_eq!(editor.document().content.sources, imported.content.sources);
    assert!(editor.document().content.next_id >= highwater);
    editor.undo(editor.revision().document).unwrap().unwrap();
    let second = editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::ImportMidi {
                tracks: vec![track("second")],
                at: Ticks::ZERO,
            }],
        )
        .unwrap();
    assert!(first.created.iter().all(|id| !second.created.contains(id)));
    assert!(editor.document().content.next_id > highwater);
}

#[test]
fn import_does_not_mutate_existing_shared_branch_sources() {
    let mut editor = editor();
    let group = editor.document().content.tracks[0].group;
    editor
        .apply(
            0,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::DuplicateBranch { group }],
        )
        .unwrap();
    let old_sources = editor.document().content.sources.clone();
    let old_groups = editor.document().content.groups.clone();
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            Edit::ExternalChanges,
            &[NativeEdit::ImportMidi {
                tracks: vec![track("new")],
                at: Ticks::ZERO,
            }],
        )
        .unwrap();
    assert!(editor.document().content.sources.starts_with(&old_sources));
    assert!(editor.document().content.groups.starts_with(&old_groups));
}

#[test]
fn oversized_import_payload_is_rejected_before_document_clone_commit() {
    let mut editor = editor();
    let before = editor.document().clone();
    let oversized = PreparedMidiTrack {
        name: "x".repeat((1 << 20) + 4096),
        ..track("oversized")
    };
    let result = editor.apply(
        0,
        NativeEditOwner::Human,
        Edit::ExternalChanges,
        &[NativeEdit::ImportMidi {
            tracks: vec![oversized],
            at: Ticks::ZERO,
        }],
    );
    assert!(matches!(result, Err(NativeEditError::Content(_))));
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.revision().document, 0);
    assert!(!editor.can_undo());
}
