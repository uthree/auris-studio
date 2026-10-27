use std::collections::BTreeMap;

use auris_core::pipeline::content::{
    ContentError, ContentLimits, PipelineContent, ProcessingGroup, Processor, ProcessorKind,
    SourceContent, SourceDocument, SourceDocumentId, StripState, TrackBinding,
};
use auris_core::pipeline::document::{
    NativeDocument, NativeProjectContext, NativeTrackKind, TrackPresentation,
};
use auris_core::project::{AudioSource, Color, Project, SoundFontId, SoundFontRef};
use auris_core::{AssetPath, SourceId, TrackId};

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 8,
        clips: 8,
        stages: 8,
        processors: 16,
        groups: 8,
        tracks: 8,
        edges: 8,
        external_ids: 8,
        automations: 0,
        automation_points: 0,
        bindings: 0,
    }
}

fn document() -> NativeDocument {
    let project = Project::default();
    let source_next_id = project.next_id();
    let mut content = PipelineContent::new();
    content.next_id = 100;
    content.sources.push(SourceDocument {
        id: SourceDocumentId(10),
        content: SourceContent::Audio(Vec::new()),
    });
    content.groups.push(ProcessingGroup {
        id: auris_core::pipeline::content::GroupId(40),
        strip: auris_core::pipeline::NodeId(41),
        primary_instrument: None,
    });
    content.processors.push(Processor {
        id: auris_core::pipeline::NodeId(41),
        group: Some(auris_core::pipeline::content::GroupId(40)),
        kind: ProcessorKind::Strip(StripState::default()),
    });
    content.tracks.push(TrackBinding {
        track: TrackId(50),
        source: Some(SourceDocumentId(10)),
        group: auris_core::pipeline::content::GroupId(40),
    });
    NativeDocument {
        context: NativeProjectContext {
            source_format_version: project.format_version,
            saved_by: project.saved_by,
            name: project.name,
            sample_rate: project.sample_rate,
            tempo_map: project.tempo_map,
            signatures: project.signatures,
            harmony: project.harmony,
            sections: project.sections,
            audio_sources: BTreeMap::new(),
            soundfonts: BTreeMap::new(),
            loop_region: project.loop_region,
            loop_enabled: project.loop_enabled,
            punch_region: project.punch_region,
            punch_enabled: project.punch_enabled,
            metronome: project.metronome,
            count_in_bars: project.count_in_bars,
            grid: project.grid,
            song_spec: project.song_spec,
            creation_brief: project.creation_brief,
            composition_baseline: project.composition_baseline,
            transcription_provenance: project.transcription_provenance,
            mixer_graph_view: project.mixer_graph_view,
            source_next_id,
        },
        track_presentation: vec![TrackPresentation {
            id: TrackId(50),
            name: "Audio".into(),
            color: Color(0x123456),
            height: 80.0,
            kind: NativeTrackKind::Audio,
        }],
        content,
    }
}

#[test]
fn valid_one_to_one_presentation_and_binding_passes() {
    assert!(document().validate_ownership(limits()).is_ok());
}

#[test]
fn duplicate_or_unmatched_presentations_are_rejected() {
    let mut native = document();
    native
        .track_presentation
        .push(native.track_presentation[0].clone());
    assert!(matches!(
        native.validate_ownership(limits()),
        Err(ContentError::DuplicateId(50))
    ));

    let mut native = document();
    native.track_presentation[0].id = TrackId(999);
    assert!(matches!(
        native.validate_ownership(limits()),
        Err(ContentError::Missing(_))
    ));
}

#[test]
fn mismatched_source_kind_and_primary_role_are_rejected() {
    let mut native = document();
    native.track_presentation[0].kind = NativeTrackKind::Instrument;
    assert!(matches!(
        native.validate_ownership(limits()),
        Err(ContentError::Role(_, _))
    ));

    let mut native = document();
    native.content.groups[1].primary_instrument = Some(auris_core::pipeline::NodeId(41));
    assert!(matches!(
        native.validate_ownership(limits()),
        Err(ContentError::Role(_, _))
    ));
}

#[test]
fn singer_presentation_requires_a_singer_processor() {
    let mut native = document();
    native.content.sources[0].content = SourceContent::Notes(Vec::new());
    native.content.groups[1].primary_instrument = Some(auris_core::pipeline::NodeId(60));
    native.content.processors.push(Processor {
        id: auris_core::pipeline::NodeId(60),
        group: Some(auris_core::pipeline::content::GroupId(40)),
        kind: ProcessorKind::Instrument(auris_core::pipeline::content::PluginInstance {
            plugin_id: "instrument".into(),
            state: Default::default(),
            file: None,
        }),
    });
    native.track_presentation[0].kind = NativeTrackKind::Singer;
    assert!(matches!(
        native.validate_ownership(limits()),
        Err(ContentError::Role(_, message)) if message.contains("singer")
    ));
}

#[test]
fn external_registry_ids_share_the_native_namespace_and_missing_files_are_metadata_only() {
    let mut native = document();
    native.context.audio_sources.insert(
        SourceId(40),
        AudioSource {
            id: SourceId(40),
            name: "missing".into(),
            path: AssetPath::external("missing.wav"),
            frame_count: 0,
            sample_rate: 48_000.0,
            channel_count: 2,
            byte_size: 0,
            provenance: BTreeMap::new(),
        },
    );
    assert!(matches!(
        native.validate_ownership(limits()),
        Err(ContentError::DuplicateId(40))
    ));
}

#[test]
fn audio_and_font_external_ids_cannot_collide() {
    let mut native = document();
    native.context.audio_sources.insert(
        SourceId(70),
        AudioSource {
            id: SourceId(70),
            name: "audio".into(),
            path: AssetPath::external("missing.wav"),
            frame_count: 0,
            sample_rate: 48_000.0,
            channel_count: 2,
            byte_size: 0,
            provenance: BTreeMap::new(),
        },
    );
    native.context.soundfonts.insert(
        SoundFontId(70),
        SoundFontRef {
            id: SoundFontId(70),
            name: "font".into(),
            path: AssetPath::external("missing.sf2"),
            byte_size: 0,
        },
    );
    assert!(matches!(
        native.validate_ownership(limits()),
        Err(ContentError::DuplicateId(70))
    ));
}

#[test]
fn limits_are_checked_before_deep_validation() {
    let mut native = document();
    native.context.audio_sources.insert(
        SourceId(40),
        AudioSource {
            id: SourceId(999),
            name: "malformed".into(),
            path: AssetPath::external("missing.wav"),
            frame_count: 0,
            sample_rate: 48_000.0,
            channel_count: 2,
            byte_size: 0,
            provenance: BTreeMap::new(),
        },
    );
    let mut constrained = limits();
    constrained.external_ids = 0;
    assert!(matches!(
        native.validate_ownership(constrained),
        Err(ContentError::Limit("external_ids"))
    ));
}

#[test]
fn native_document_round_trips_through_serde() {
    let native = document();
    let json = serde_json::to_string(&native).unwrap();
    let decoded: NativeDocument = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, native);
    assert!(decoded.validate_ownership(limits()).is_ok());
}
