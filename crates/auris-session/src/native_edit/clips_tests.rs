use super::NativeClipEdit;
use crate::native_edit::NativeCreated;
use crate::native_edit::clips::apply;
use auris_core::pipeline::content::{ContentLimits, SourceContent, SourceDocumentId};
use auris_core::project::{NoteTransform, Project};
use auris_core::time::Ticks;

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 16,
        clips: 64,
        stages: 64,
        processors: 64,
        groups: 16,
        tracks: 8,
        edges: 64,
        external_ids: 16,
        automations: 8,
        automation_points: 32,
        bindings: 16,
    }
}

fn document() -> (
    auris_core::pipeline::document::NativeDocument,
    SourceDocumentId,
) {
    let mut project = Project::new("clips", 48_000.0);
    project.add_instrument_track("Synth", "auris.synth.chiptune");
    let document = auris_io::project_to_native(&project, limits()).expect("native");
    let source = document
        .content
        .sources
        .iter()
        .find(|source| matches!(source.content, SourceContent::Notes(_)))
        .expect("notes")
        .id;
    (document, source)
}

#[test]
fn add_move_rename_mute_and_resize_use_stable_source_addresses() {
    let (mut document, source) = document();
    let created = apply(
        &mut document,
        &NativeClipEdit::AddMidi {
            source,
            name: "phrase".into(),
            start: Ticks::ZERO,
            length: Ticks::QUARTER,
        },
        limits(),
    )
    .expect("add");
    let clip = match created[0] {
        NativeCreated::Clip(id) => id,
        _ => panic!("clip"),
    };
    apply(
        &mut document,
        &NativeClipEdit::Move {
            source,
            clip,
            start: Ticks::QUARTER,
        },
        limits(),
    )
    .expect("move");
    apply(
        &mut document,
        &NativeClipEdit::Rename {
            source,
            clip,
            name: "verse".into(),
        },
        limits(),
    )
    .expect("rename");
    apply(
        &mut document,
        &NativeClipEdit::SetMuted {
            source,
            clip,
            muted: true,
        },
        limits(),
    )
    .expect("mute");
    apply(
        &mut document,
        &NativeClipEdit::Resize {
            source,
            clip,
            length: Ticks::QUARTER * 2,
        },
        limits(),
    )
    .expect("resize");
    let SourceContent::Notes(clips) = &document
        .content
        .sources
        .iter()
        .find(|entry| entry.id == source)
        .unwrap()
        .content
    else {
        panic!("notes")
    };
    assert_eq!(clips[0].name, "verse");
    assert!(clips[0].muted);
    assert_eq!(clips[0].length, Ticks::QUARTER * 2);
}

#[test]
fn duplicate_allocates_fresh_clip_and_stage_ids() {
    let (mut document, source) = document();
    let first = apply(
        &mut document,
        &NativeClipEdit::AddMidi {
            source,
            name: "phrase".into(),
            start: Ticks::ZERO,
            length: Ticks::QUARTER,
        },
        limits(),
    )
    .expect("add");
    let clip = match first[0] {
        NativeCreated::Clip(id) => id,
        _ => panic!("clip"),
    };
    let mut editor = crate::native_edit::NativeDocumentEditor::new(document, limits()).unwrap();
    editor
        .apply(
            0,
            crate::native_edit::NativeEditOwner::Human,
            crate::Edit::ExternalChanges,
            &[
                super::NativeEdit::AppendStage {
                    clip,
                    transform: NoteTransform::Gate { amount: 0.2 },
                },
                super::NativeEdit::AppendStage {
                    clip,
                    transform: NoteTransform::Transpose { semitones: 12 },
                },
            ],
        )
        .unwrap();
    let receipt = editor
        .apply(
            1,
            crate::native_edit::NativeEditOwner::Human,
            crate::Edit::ExternalChanges,
            &[super::NativeEdit::Clip(NativeClipEdit::Duplicate {
                source,
                clip,
            })],
        )
        .unwrap();
    assert!(matches!(
        receipt.created.as_slice(),
        [
            NativeCreated::Clip(_),
            NativeCreated::Stage(_),
            NativeCreated::Stage(_)
        ]
    ));
    let duplicated = editor.document().clone();
    let SourceContent::Notes(clips) = &duplicated
        .content
        .sources
        .iter()
        .find(|entry| entry.id == source)
        .unwrap()
        .content
    else {
        panic!("notes")
    };
    assert_eq!(clips.len(), 2);
    assert_ne!(clips[0].id, clips[1].id);
    assert_eq!(clips[0].transforms.len(), clips[1].transforms.len());
    assert_ne!(clips[0].transforms[0].id, clips[1].transforms[0].id);
    assert_ne!(clips[0].transforms[1].id, clips[1].transforms[1].id);
    assert_eq!(
        clips[0].performer_clip().transforms,
        clips[1].performer_clip().transforms
    );
    editor.undo(2).unwrap();
    editor.redo(3).unwrap();
    assert_eq!(editor.document(), &duplicated);
}

#[test]
fn invalid_late_clip_edit_is_rollback_safe_for_batch_candidate() {
    let (document, source) = document();
    let mut editor =
        crate::native_edit::NativeDocumentEditor::new(document, limits()).expect("editor");
    let invalid = NativeClipEdit::AddMidi {
        source,
        name: "".into(),
        start: Ticks::ZERO,
        length: Ticks::QUARTER,
    };
    let valid = NativeClipEdit::AddMidi {
        source,
        name: "ok".into(),
        start: Ticks::ZERO,
        length: Ticks::QUARTER,
    };
    let before = editor.document().clone();
    let result = editor.apply(
        0,
        crate::native_edit::NativeEditOwner::Human,
        crate::Edit::ExternalChanges,
        &[
            crate::native_edit::NativeEdit::Clip(valid),
            crate::native_edit::NativeEdit::Clip(invalid),
        ],
    );
    assert!(result.is_err());
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.revision().document, 0);
    assert!(!editor.can_undo());
}

#[test]
fn audio_clips_support_move_duplicate_mute_rename_and_remove_with_provenance() {
    let mut project = Project::new("audio clips", 48_000.0);
    let track = project.add_audio_track("Audio");
    let source = project.add_audio_source(
        "take",
        auris_core::AssetPath::external("take.wav"),
        16,
        48_000.0,
        1,
    );
    let clip = project
        .add_audio_clip(track, source, Ticks::ZERO)
        .expect("clip");
    let mut document = auris_io::project_to_native(&project, limits()).expect("native");
    let source_id = document
        .content
        .sources
        .iter()
        .find(|entry| matches!(entry.content, SourceContent::Audio(_)))
        .expect("audio source")
        .id;
    document
        .context
        .transcription_provenance
        .insert(clip, "captured".into());
    apply(
        &mut document,
        &NativeClipEdit::Move {
            source: source_id,
            clip,
            start: Ticks::QUARTER,
        },
        limits(),
    )
    .expect("move");
    apply(
        &mut document,
        &NativeClipEdit::SetMuted {
            source: source_id,
            clip,
            muted: true,
        },
        limits(),
    )
    .expect("mute");
    apply(
        &mut document,
        &NativeClipEdit::Rename {
            source: source_id,
            clip,
            name: "renamed".into(),
        },
        limits(),
    )
    .expect("rename");
    let created = apply(
        &mut document,
        &NativeClipEdit::Duplicate {
            source: source_id,
            clip,
        },
        limits(),
    )
    .expect("duplicate");
    let copy = match created[0] {
        NativeCreated::Clip(id) => id,
        _ => panic!("clip"),
    };
    assert_eq!(
        document
            .context
            .transcription_provenance
            .get(&copy)
            .map(String::as_str),
        Some("captured")
    );
    apply(
        &mut document,
        &NativeClipEdit::Remove {
            source: source_id,
            clip,
        },
        limits(),
    )
    .expect("remove");
    assert!(
        !document
            .context
            .transcription_provenance
            .contains_key(&clip)
    );
}
