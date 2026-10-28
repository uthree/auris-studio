use super::NativeAudioEdit;
use super::audio::apply;
use crate::native_edit::{NativeCreated, NativeDocumentEditor, NativeEditOwner};
use auris_core::AssetPath;
use auris_core::pipeline::content::ContentLimits;
use auris_core::project::Project;
use auris_core::time::Ticks;

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 8,
        clips: 32,
        stages: 64,
        processors: 128,
        groups: 16,
        tracks: 8,
        edges: 256,
        external_ids: 32,
        automations: 16,
        automation_points: 128,
        bindings: 32,
    }
}

fn edit(track: auris_core::TrackId) -> NativeAudioEdit {
    NativeAudioEdit {
        track,
        name: "take".into(),
        path: AssetPath::external("take.wav"),
        frame_count: 48_000,
        sample_rate: 48_000.0,
        channel_count: 2,
        byte_size: 192_000,
        provenance: Default::default(),
        start: Ticks::ZERO,
        offset_frames: 0,
        length_frames: 48_000,
        gain_db: 0.0,
        source_bpm: None,
        follows_tempo: false,
    }
}

fn document() -> (
    auris_core::pipeline::document::NativeDocument,
    auris_core::TrackId,
) {
    let mut project = Project::new("native audio", 48_000.0);
    let track = project.add_audio_track("Audio");
    let document = auris_io::project_to_native(&project, limits()).expect("native conversion");
    (document, track)
}

fn two_audio_document() -> (
    auris_core::pipeline::document::NativeDocument,
    auris_core::TrackId,
    auris_core::TrackId,
) {
    let mut project = Project::new("native audio", 48_000.0);
    let first = project.add_audio_track("First");
    let second = project.add_audio_track("Second");
    let document = auris_io::project_to_native(&project, limits()).expect("native conversion");
    (document, first, second)
}

#[test]
fn landing_allocates_source_and_clip_without_caller_ids() {
    let (mut document, track) = document();
    let before = document.clone();
    let created = apply(&mut document, &edit(track), limits()).expect("audio landing");
    assert!(matches!(
        created.as_slice(),
        [NativeCreated::AudioSource(_), NativeCreated::Clip(_)]
    ));
    assert_ne!(document, before);
    let source = document
        .context
        .audio_sources
        .values()
        .next()
        .expect("source metadata");
    assert_eq!(source.frame_count, 48_000);
    assert_eq!(source.channel_count, 2);
}

#[test]
fn invalid_late_take_is_atomic() {
    let (mut document, track) = document();
    let before = document.clone();
    let mut invalid = edit(track);
    invalid.length_frames = invalid.frame_count + 1;
    assert!(apply(&mut document, &invalid, limits()).is_err());
    assert_eq!(document, before);
}

#[test]
fn recorded_batch_rejects_late_failure_without_consuming_ids() {
    let (document, first, second) = two_audio_document();
    let mut editor = NativeDocumentEditor::new(document, limits()).expect("editor");
    let before = editor.document().clone();
    let mut bad = edit(second);
    bad.length_frames = bad.frame_count + 1;
    let error = editor.land_recorded_audio(0, NativeEditOwner::Human, &[edit(first), bad]);
    assert!(error.is_err());
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.revision().document, 0);
    assert!(!editor.can_undo());
}

#[test]
fn recorded_batch_rejects_ambiguous_duplicate_target() {
    let (document, track) = document();
    let mut editor = NativeDocumentEditor::new(document, limits()).expect("editor");
    let error = editor.land_recorded_audio(0, NativeEditOwner::Human, &[edit(track), edit(track)]);
    assert!(error.is_err());
    assert!(editor.document().context.audio_sources.is_empty());
}

#[test]
fn recorded_batch_has_one_undo_step_and_preserves_created_ids_on_redo() {
    let (document, track) = document();
    let mut editor = NativeDocumentEditor::new(document, limits()).expect("editor");
    let receipt = editor
        .land_recorded_audio(0, NativeEditOwner::Human, &[edit(track)])
        .expect("land");
    assert!(matches!(
        receipt.created.as_slice(),
        [NativeCreated::AudioSource(_), NativeCreated::Clip(_)]
    ));
    let created = receipt.created.clone();
    let revision = receipt.revision.document;
    let _ = editor.undo(revision).expect("undo").expect("undo receipt");
    let redo = editor
        .redo(editor.revision().document)
        .expect("redo")
        .expect("redo receipt");
    assert!(redo.changed);
    assert_eq!(editor.document().context.audio_sources.len(), 1);
    assert!(matches!(created[0], NativeCreated::AudioSource(_)));
    assert!(matches!(created[1], NativeCreated::Clip(_)));
    assert!(
        editor
            .undo(editor.revision().document)
            .expect("undo")
            .expect("receipt")
            .changed
    );
    assert_eq!(editor.revision().musical, 4);
}
