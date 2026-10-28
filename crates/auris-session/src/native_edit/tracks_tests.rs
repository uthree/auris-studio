use super::*;
use auris_core::Project as CoreProject;
use auris_core::TrackId;
use auris_core::pipeline::content::{ContentLimits, PluginInstance};
use auris_core::pipeline::document::NativeTrackKind;
use auris_core::plugin::PluginState;

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
    let project = CoreProject::new("tracks", 48_000.0);
    NativeDocumentEditor::new(
        auris_io::project_to_native(&project, limits()).unwrap(),
        limits(),
    )
    .unwrap()
}

fn instrument() -> NativeTrackKindSpec {
    NativeTrackKindSpec::Instrument(PluginInstance {
        plugin_id: "builtin:test".into(),
        state: PluginState::empty(),
        file: None,
    })
}

fn add(editor: &mut NativeDocumentEditor, kind: NativeTrackKindSpec, name: &str) -> TrackId {
    let receipt = editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::AddInstrumentTrack,
            &[NativeEdit::Track(NativeTrackEdit::Add {
                name: name.into(),
                kind,
            })],
        )
        .unwrap();
    receipt
        .created
        .into_iter()
        .find_map(|created| match created {
            NativeCreated::Track(track) => Some(track),
            _ => None,
        })
        .unwrap()
}

#[test]
fn adds_audio_bus_and_instrument_tracks_with_expected_roles() {
    let mut editor = editor();
    let audio = add(&mut editor, NativeTrackKindSpec::Audio, "Audio");
    let bus = add(&mut editor, NativeTrackKindSpec::Bus, "Bus");
    let instrument = add(&mut editor, instrument(), "Instrument");

    assert_eq!(
        editor
            .document()
            .track_presentation
            .iter()
            .filter(|entry| matches!(entry.kind, NativeTrackKind::Audio))
            .count(),
        1
    );
    assert!(
        editor
            .document()
            .content
            .tracks
            .iter()
            .any(|entry| entry.track == audio && entry.source.is_some())
    );
    assert!(
        editor
            .document()
            .content
            .tracks
            .iter()
            .any(|entry| entry.track == bus && entry.source.is_none())
    );
    assert!(
        editor
            .document()
            .content
            .tracks
            .iter()
            .any(|entry| entry.track == instrument)
    );
}

#[test]
fn move_reorders_presentation_and_bindings_together() {
    let mut editor = editor();
    let first = add(&mut editor, NativeTrackKindSpec::Audio, "First");
    let second = add(&mut editor, NativeTrackKindSpec::Audio, "Second");
    let revision = editor.revision().document;
    editor
        .apply(
            revision,
            NativeEditOwner::Human,
            crate::Edit::MoveTrack,
            &[NativeEdit::Track(NativeTrackEdit::Move {
                track: first,
                index: 1,
            })],
        )
        .unwrap();
    assert_eq!(
        editor.document().track_presentation.last().unwrap().id,
        first
    );
    assert_eq!(
        editor.document().content.tracks.last().unwrap().track,
        first
    );
    assert_ne!(first, second);
}

#[test]
fn remove_with_pruning_off_preserves_source_document() {
    let mut editor = editor();
    let track = add(&mut editor, NativeTrackKindSpec::Audio, "Audio");
    let source = editor
        .document()
        .content
        .tracks
        .iter()
        .find(|entry| entry.track == track)
        .and_then(|entry| entry.source)
        .unwrap();
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::DeleteTrack,
            &[NativeEdit::Track(NativeTrackEdit::Remove {
                track,
                prune_source: false,
            })],
        )
        .unwrap();
    assert!(
        editor
            .document()
            .content
            .sources
            .iter()
            .any(|entry| entry.id == source)
    );
}

#[test]
fn remove_never_leaves_dangling_route_endpoints() {
    let mut editor = editor();
    let bus = add(&mut editor, NativeTrackKindSpec::Bus, "Bus");
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::DeleteTrack,
            &[NativeEdit::Track(NativeTrackEdit::Remove {
                track: bus,
                prune_source: true,
            })],
        )
        .unwrap();
    let nodes: std::collections::BTreeSet<_> = editor
        .document()
        .content
        .processors
        .iter()
        .map(|node| node.id)
        .collect();
    assert!(
        editor
            .document()
            .content
            .edges
            .iter()
            .all(|edge| nodes.contains(&edge.from.0) && nodes.contains(&edge.to.0))
    );
}

#[test]
fn undo_preserves_id_high_water_mark_after_track_deletion() {
    let mut editor = editor();
    let track = add(&mut editor, NativeTrackKindSpec::Audio, "Audio");
    let high_water = editor.document().content.next_id;
    editor
        .apply(
            editor.revision().document,
            NativeEditOwner::Human,
            crate::Edit::DeleteTrack,
            &[NativeEdit::Track(NativeTrackEdit::Remove {
                track,
                prune_source: true,
            })],
        )
        .unwrap();
    editor.undo(editor.revision().document).unwrap();
    assert!(editor.document().content.next_id >= high_water);
}
