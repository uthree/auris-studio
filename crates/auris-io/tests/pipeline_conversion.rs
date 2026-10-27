use auris_core::pipeline::content::ContentLimits;
use auris_core::pipeline::content::{PluginInstance, ProcessorKind, SourceContent};
use auris_io::{load_project, project_to_native};
use std::fs;

const FIXTURES: [&str; 6] = [
    include_str!("fixtures/project_formats/v27.auris"),
    include_str!("fixtures/project_formats/v28.auris"),
    include_str!("fixtures/project_formats/v29.auris"),
    include_str!("fixtures/project_formats/v30.auris"),
    include_str!("fixtures/project_formats/v31.auris"),
    include_str!("fixtures/project_formats/v32.auris"),
];

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 64,
        clips: 1024,
        stages: 4096,
        processors: 4096,
        groups: 256,
        tracks: 256,
        edges: 8192,
        external_ids: 1024,
        automations: 256,
        automation_points: 8192,
        bindings: 1024,
    }
}

fn fixture_project(text: &str) -> auris_core::Project {
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), text).unwrap();
    load_project(file.path()).unwrap()
}

#[test]
fn every_historical_fixture_converts_without_loading_plugins() {
    for text in FIXTURES {
        let project = fixture_project(text);
        let native = project_to_native(&project, limits()).unwrap();
        assert_eq!(native.context.name, project.name);
        assert_eq!(native.context.saved_by, project.saved_by);
        assert_eq!(native.context.sample_rate, project.sample_rate);
        assert_eq!(native.context.tempo_map, project.tempo_map);
        assert_eq!(native.context.signatures, project.signatures);
        assert_eq!(native.context.harmony, project.harmony);
        assert_eq!(native.context.sections, project.sections);
        assert_eq!(native.context.loop_region, project.loop_region);
        assert_eq!(native.context.loop_enabled, project.loop_enabled);
        assert_eq!(native.context.punch_region, project.punch_region);
        assert_eq!(native.context.punch_enabled, project.punch_enabled);
        assert_eq!(native.context.metronome, project.metronome);
        assert_eq!(native.context.count_in_bars, project.count_in_bars);
        assert_eq!(native.context.grid, project.grid);
        assert_eq!(native.context.song_spec, project.song_spec);
        assert_eq!(native.context.creation_brief, project.creation_brief);
        assert_eq!(
            native.context.composition_baseline,
            project.composition_baseline
        );
        assert_eq!(native.context.mixer_graph_view, project.mixer_graph_view);
        assert_eq!(native.context.audio_sources, project.audio_sources);
        assert_eq!(native.context.soundfonts, project.soundfonts);
        assert_eq!(
            native.context.transcription_provenance,
            project.transcription_provenance
        );
        assert_eq!(native.track_presentation.len(), project.tracks.len());
        assert_eq!(native.content.tracks.len(), project.tracks.len());

        for track in &project.tracks {
            let binding = native
                .content
                .tracks
                .iter()
                .find(|binding| binding.track == track.id)
                .unwrap();
            if let Some(legacy_clips) = track.kind.note_clips() {
                let source = native
                    .content
                    .sources
                    .iter()
                    .find(|source| Some(source.id) == binding.source)
                    .unwrap();
                let SourceContent::Notes(native_clips) = &source.content else {
                    panic!("note source became audio");
                };
                assert_eq!(
                    native_clips
                        .iter()
                        .map(|clip| clip.performer_clip())
                        .collect::<Vec<_>>(),
                    *legacy_clips
                );
            }
            if let Some(audio) = track.kind.as_audio() {
                let source = native
                    .content
                    .sources
                    .iter()
                    .find(|source| Some(source.id) == binding.source)
                    .unwrap();
                let SourceContent::Audio(native_clips) = &source.content else {
                    panic!("audio source became notes");
                };
                assert_eq!(native_clips, &audio.clips);
            }
        }

        for track in &project.tracks {
            let effects: Vec<_> = native
                .content
                .processors
                .iter()
                .filter_map(|processor| match &processor.kind {
                    ProcessorKind::Effect {
                        slot,
                        plugin:
                            PluginInstance {
                                plugin_id,
                                state,
                                file,
                            },
                        enabled,
                    } if track.mixer.effects.iter().any(|effect| effect.id == *slot) => {
                        Some((slot, plugin_id, state, file, enabled))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(effects.len(), track.mixer.effects.len());
            for effect in &track.mixer.effects {
                let (_, plugin_id, state, file, enabled) = effects
                    .iter()
                    .find(|(slot, ..)| **slot == effect.id)
                    .unwrap();
                assert_eq!(*plugin_id, &effect.effect_id);
                assert_eq!(*state, &effect.state);
                assert_eq!(*file, &effect.file);
                assert_eq!(**enabled, effect.enabled);
            }
        }
    }
}

#[test]
fn conversion_is_deterministic_and_round_trips_as_native_json() {
    let project = fixture_project(FIXTURES[5]);
    let first = project_to_native(&project, limits()).unwrap();
    let second = project_to_native(&project, limits()).unwrap();
    assert_eq!(first, second);
    let json = serde_json::to_string(&first).unwrap();
    let decoded: auris_core::pipeline::document::NativeDocument =
        serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, first);
}

#[test]
fn native_ids_and_automation_payloads_are_stable_and_complete() {
    let project = fixture_project(FIXTURES[5]);
    let native = project_to_native(&project, limits()).unwrap();
    assert!(native.content.next_id > native.context.source_next_id);
    assert_eq!(native.context.source_next_id, project.next_id());
    assert_eq!(
        native.content.automation.len(),
        project.automation.lanes().len()
    );
    for (legacy, curve) in project
        .automation
        .lanes()
        .iter()
        .zip(&native.content.automation)
    {
        assert_eq!(curve.points(), legacy.points());
        assert_eq!(curve.curve, legacy.curve);
        if legacy.target.is_builtin() {
            assert!(curve.domain.is_some());
        } else {
            assert!(curve.domain.is_none());
        }
    }
    let route_edges = &native.content.edges;
    assert!(route_edges.iter().any(|edge| edge.from.1.0 == "pre_fader"));
    assert!(
        route_edges
            .iter()
            .any(|edge| edge.role == auris_core::pipeline::EdgeRole::Sidechain)
    );
    assert!(
        native
            .content
            .processors
            .iter()
            .filter(|processor| matches!(&processor.kind, ProcessorKind::Sum))
            .count()
            >= project.tracks.len()
    );
    assert!(
        native
            .content
            .processors
            .iter()
            .any(|processor| matches!(&processor.kind, ProcessorKind::Output))
    );
    let singer = project
        .tracks
        .iter()
        .find(|track| matches!(track.kind, auris_core::project::TrackKind::Singer(_)))
        .unwrap();
    let singer_binding = native
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == singer.id)
        .unwrap();
    let singer_group = native
        .content
        .groups
        .iter()
        .find(|group| group.id == singer_binding.group)
        .unwrap();
    let guide = singer_group.primary_instrument.unwrap();
    let singer_node = native
        .content
        .processors
        .iter()
        .find(|processor| {
            processor.group == Some(singer_group.id)
                && matches!(&processor.kind, ProcessorKind::Singer(_))
        })
        .unwrap()
        .id;
    assert!(
        route_edges.iter().any(|edge| edge.from.0 == guide
            && edge.to.0 == singer_node
            && edge.to.1.0 == "guide_in")
    );
}

#[test]
fn caller_budget_rejects_before_conversion() {
    let project = fixture_project(FIXTURES[5]);
    let mut constrained = limits();
    constrained.tracks = 1;
    let error = project_to_native(&project, constrained).unwrap_err();
    assert!(error.to_string().contains("tracks"));
}

#[test]
fn malformed_routing_is_rejected_without_mutating_the_legacy_project() {
    let mut project = fixture_project(FIXTURES[5]);
    let track = project
        .tracks
        .iter_mut()
        .find(|track| !track.kind.is_bus())
        .unwrap();
    track.outputs = vec![auris_core::project::Output::Bus(auris_core::TrackId(
        9_999_999,
    ))];
    let before = project.clone();
    assert!(project_to_native(&project, limits()).is_err());
    assert_eq!(project, before);
}

#[test]
fn missing_audio_source_reference_is_retained_without_changing_the_input() {
    let mut project = fixture_project(FIXTURES[5]);
    let audio = project
        .tracks
        .iter_mut()
        .find_map(|track| track.kind.as_audio_mut())
        .unwrap();
    audio.clips[0].source = auris_core::SourceId(8_888_888);
    let candidate = project.clone();
    let native = project_to_native(&project, limits()).unwrap();
    assert!(native.content.sources.iter().any(|source| matches!(
        &source.content, SourceContent::Audio(clips) if clips.iter().any(|clip| clip.source == auris_core::SourceId(8_888_888))
    )));
    assert_eq!(project, candidate);
}
