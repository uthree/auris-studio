use auris_core::param::{ParamId, ParamTarget};
use auris_core::pipeline::NodeId;
use auris_core::pipeline::content::{
    ContentError, ContentLimits, GroupId, PipelineContent, PluginInstance, ProcessingGroup,
    Processor, ProcessorKind, SourceContent, SourceDocument, SourceDocumentId, StripState,
    TrackBinding,
};
use auris_core::project::{
    AudioClip, ClipId, FadeCurve, MidiClip, NoteTransform, SourceId, StageId,
};
use auris_core::{EffectSlotId, PluginState, SendId, Ticks, TrackId};

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 8,
        clips: 32,
        stages: 64,
        processors: 32,
        groups: 8,
        tracks: 16,
        edges: 32,
        external_ids: 0,
    }
}

fn plugin(name: &str) -> PluginInstance {
    PluginInstance {
        plugin_id: name.into(),
        state: PluginState::default(),
        file: None,
    }
}

fn fixture() -> PipelineContent {
    let mut legacy = MidiClip::new(ClipId(20), "phrase", Ticks::ZERO, Ticks(480));
    legacy.transforms = vec![
        NoteTransform::Transpose { semitones: 12 },
        NoteTransform::Lean { ticks: 10 },
    ];
    let clip = legacy
        .with_stage_ids(vec![StageId(30), StageId(31)])
        .unwrap();
    let mut content = PipelineContent::new();
    content.sources.push(SourceDocument {
        id: SourceDocumentId(10),
        content: SourceContent::Notes(vec![clip]),
    });
    content.groups.extend([
        ProcessingGroup {
            id: GroupId(100),
            strip: NodeId(101),
            primary_instrument: Some(NodeId(102)),
        },
        ProcessingGroup {
            id: GroupId(110),
            strip: NodeId(111),
            primary_instrument: Some(NodeId(112)),
        },
    ]);
    content.processors.extend([
        Processor {
            id: NodeId(101),
            group: Some(GroupId(100)),
            kind: ProcessorKind::Strip(StripState::default()),
        },
        Processor {
            id: NodeId(102),
            group: Some(GroupId(100)),
            kind: ProcessorKind::Instrument(plugin("one")),
        },
        Processor {
            id: NodeId(103),
            group: Some(GroupId(100)),
            kind: ProcessorKind::Instrument(plugin("guide")),
        },
        Processor {
            id: NodeId(104),
            group: Some(GroupId(100)),
            kind: ProcessorKind::Effect {
                slot: EffectSlotId(40),
                plugin: plugin("fx"),
                enabled: true,
            },
        },
        Processor {
            id: NodeId(105),
            group: Some(GroupId(100)),
            kind: ProcessorKind::Send {
                send: SendId(50),
                level_db: 0.,
            },
        },
        Processor {
            id: NodeId(111),
            group: Some(GroupId(110)),
            kind: ProcessorKind::Strip(StripState::default()),
        },
        Processor {
            id: NodeId(112),
            group: Some(GroupId(110)),
            kind: ProcessorKind::Instrument(plugin("two")),
        },
        Processor {
            id: NodeId(106),
            group: Some(GroupId(1)),
            kind: ProcessorKind::Effect {
                slot: EffectSlotId(60),
                plugin: plugin("master-fx"),
                enabled: true,
            },
        },
    ]);
    content.tracks.extend([
        TrackBinding {
            track: TrackId(200),
            source: Some(SourceDocumentId(10)),
            group: GroupId(100),
        },
        TrackBinding {
            track: TrackId(201),
            source: Some(SourceDocumentId(10)),
            group: GroupId(110),
        },
    ]);
    content.next_id = 1000;
    content
}

#[test]
fn shared_source_and_explicit_primary_are_resolved_without_order_fallback() {
    let content = fixture();
    let bindings = content.command_bindings(&[], limits()).unwrap();
    assert_eq!(bindings.source(TrackId(200)).unwrap(), SourceDocumentId(10));
    assert_eq!(bindings.source(TrackId(201)).unwrap(), SourceDocumentId(10));
    assert_eq!(bindings.clip(ClipId(20)).unwrap(), SourceDocumentId(10));
    assert_eq!(bindings.instrument(TrackId(200)).unwrap(), NodeId(102));
    assert_eq!(bindings.instrument(TrackId(201)).unwrap(), NodeId(112));
}

#[test]
fn stages_keep_identity_when_local_order_changes() {
    let mut content = fixture();
    let bindings = content.command_bindings(&[], limits()).unwrap();
    assert_eq!(bindings.stage(ClipId(20), StageId(31)).unwrap().1, 1);
    if let SourceContent::Notes(clips) = &mut content.sources[0].content {
        clips[0].move_stage(StageId(31), -1).unwrap();
    }
    let bindings = content.command_bindings(&[], limits()).unwrap();
    assert_eq!(bindings.stage(ClipId(20), StageId(31)).unwrap().1, 0);
}

#[test]
fn conventional_parameters_use_builtin_keys_and_plugin_stable_keys() {
    let content = fixture();
    let bindings = content.command_bindings(&[], limits()).unwrap();
    assert_eq!(
        bindings
            .parameter(ParamTarget::TrackGain(TrackId(200)), None)
            .unwrap()
            .key,
        "gain_db"
    );
    assert_eq!(
        bindings
            .parameter(ParamTarget::TrackPan(TrackId(200)), None)
            .unwrap()
            .key,
        "pan"
    );
    assert_eq!(
        bindings
            .parameter(ParamTarget::MasterGain, None)
            .unwrap()
            .key,
        "gain_db"
    );
    assert_eq!(
        bindings
            .parameter(ParamTarget::MasterPan, None)
            .unwrap()
            .key,
        "pan"
    );
    assert_eq!(
        bindings
            .parameter(
                ParamTarget::Send {
                    track: TrackId(200),
                    send: SendId(50)
                },
                None
            )
            .unwrap()
            .key,
        "level_db"
    );
    let target = ParamTarget::Instrument {
        track: TrackId(200),
        param: ParamId(7),
    };
    assert!(matches!(
        bindings.parameter(target, None),
        Err(ContentError::ParameterKeyRequired)
    ));
    assert_eq!(
        bindings.parameter(target, Some("cutoff_hz")).unwrap().key,
        "cutoff_hz"
    );
    let target = ParamTarget::Effect {
        track: Some(TrackId(200)),
        slot: EffectSlotId(40),
        param: ParamId(2),
    };
    assert_eq!(
        bindings.parameter(target, Some("mix")).unwrap().node,
        NodeId(104)
    );
    let target = ParamTarget::Effect {
        track: None,
        slot: EffectSlotId(60),
        param: ParamId(3),
    };
    assert_eq!(
        bindings.parameter(target, Some("ceiling_db")).unwrap().node,
        NodeId(106)
    );
}

#[test]
fn missing_primary_is_an_error_instead_of_an_order_fallback() {
    let mut content = fixture();
    content.groups[1].primary_instrument = None;
    let bindings = content.command_bindings(&[], limits()).unwrap();
    assert!(matches!(
        bindings.instrument(TrackId(200)),
        Err(ContentError::Role(..))
    ));
}

#[test]
fn audio_clip_has_no_performance_stage_address() {
    let mut content = fixture();
    content.sources[0].content = SourceContent::Audio(vec![AudioClip {
        id: ClipId(20),
        name: "audio".into(),
        start: Ticks::ZERO,
        source: SourceId(70),
        offset_frames: 0,
        length_frames: 100,
        gain_db: 0.0,
        fade_in_frames: 0,
        fade_out_frames: 0,
        fade_in_curve: FadeCurve::Linear,
        fade_out_curve: FadeCurve::Linear,
        muted: false,
        source_bpm: None,
        follows_tempo: false,
        tempo_anchor: None,
        loop_end: Ticks::ZERO,
    }]);
    let bindings = content.command_bindings(&[], limits()).unwrap();
    assert!(matches!(
        bindings.stage(ClipId(20), StageId(30)),
        Err(ContentError::Role(..))
    ));
}

#[test]
fn retained_effect_and_send_cannot_cross_track_groups() {
    let content = fixture();
    let bindings = content.command_bindings(&[], limits()).unwrap();
    assert!(matches!(
        bindings.effect(Some(TrackId(201)), EffectSlotId(40)),
        Err(ContentError::Role(..))
    ));
    assert!(matches!(
        bindings.send(TrackId(201), SendId(50)),
        Err(ContentError::Role(..))
    ));
}

#[test]
fn bindings_survive_content_serde_roundtrip() {
    let content = fixture();
    let encoded = serde_json::to_vec(&content).unwrap();
    let decoded: PipelineContent = serde_json::from_slice(&encoded).unwrap();
    let bindings = decoded.command_bindings(&[], limits()).unwrap();
    assert_eq!(bindings.clip(ClipId(20)).unwrap(), SourceDocumentId(10));
    assert_eq!(
        bindings
            .effect(Some(TrackId(200)), EffectSlotId(40))
            .unwrap(),
        NodeId(104)
    );
}
