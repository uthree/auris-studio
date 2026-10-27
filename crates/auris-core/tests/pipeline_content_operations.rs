use auris_core::pipeline::content::{
    ContentError, ContentLimits, ContentRef, GroupId, PipelineContent, PluginInstance,
    ProcessingGroup, Processor, ProcessorKind, SourceContent, SourceDocument, SourceDocumentId,
    TrackBinding,
};
use auris_core::pipeline::{EdgeId, EdgeRole, NodeId, PipelineEdge, PortId};
use auris_core::project::{
    AudioClip, AudioSource, ClipId, ClipPreset, ClipRecipe, MidiClip, Note, SourceId, StageId,
};
use auris_core::{AssetPath, TICKS_PER_QUARTER, Ticks, TrackId};

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 32,
        clips: 128,
        stages: 256,
        processors: 128,
        groups: 32,
        tracks: 128,
        edges: 256,
        external_ids: 128,
        automations: 0,
        automation_points: 0,
        bindings: 0,
    }
}

fn source() -> SourceDocument {
    let mut clip = MidiClip::new(
        ClipId(10),
        "phrase",
        Ticks::ZERO,
        Ticks(TICKS_PER_QUARTER * 4),
    );
    clip.notes
        .push(Note::new(60, Ticks::ZERO, Ticks(TICKS_PER_QUARTER)));
    clip.recipe = Some(ClipRecipe::new(ClipPreset::Lead, 4242));
    clip.transforms = vec![auris_core::project::NoteTransform::Humanize {
        amount: 0.25,
        seed: 12345,
    }];
    let clip = clip
        .with_stage_ids(vec![StageId(11)])
        .expect("fixture stage identity");
    SourceDocument {
        id: SourceDocumentId(9),
        content: SourceContent::Notes(vec![clip]),
    }
}

#[test]
fn duplicate_source_remaps_clip_and_stage_ids_without_changing_payload() {
    let mut content = PipelineContent::new();
    content.sources.push(source());
    content.next_id = 20;
    let before = content.sources[0].clone();

    let copy = content
        .duplicate_source(SourceDocumentId(9), &[], limits())
        .expect("source can be copied");

    assert_ne!(copy.source, SourceDocumentId(9));
    assert_eq!(copy.clips.get(&ClipId(10)).copied(), Some(ClipId(21)));
    assert_eq!(copy.stages.get(&StageId(11)).copied(), Some(StageId(22)));
    assert_eq!(content.sources[0], before);
    let copied = content
        .sources
        .iter()
        .find(|entry| entry.id == copy.source)
        .unwrap();
    let SourceContent::Notes(clips) = &copied.content else {
        panic!("expected note source")
    };
    assert_eq!(clips[0].id, ClipId(21));
    assert_eq!(clips[0].name, "phrase");
    let SourceContent::Notes(original) = &before.content else {
        unreachable!()
    };
    assert_eq!(clips[0].notes, original[0].notes);
    assert_eq!(clips[0].recipe, original[0].recipe);
    assert_eq!(
        clips[0].performer_clip().notes,
        original[0].performer_clip().notes
    );
    assert_eq!(clips[0].transforms[0].id, StageId(22));
    let mut restored = clips[0].clone();
    restored.id = original[0].id;
    restored.transforms[0].id = original[0].transforms[0].id;
    assert_eq!(restored, original[0]);
    let SourceContent::Notes(clips) = &mut content.sources[1].content else {
        unreachable!()
    };
    clips[0].notes[0].pitch = 72;
    assert_eq!(content.sources[0], before);
}

#[test]
fn duplicate_source_limit_failure_is_atomic() {
    let mut content = PipelineContent::new();
    content.sources.push(source());
    content.next_id = 20;
    let before = content.clone();
    let mut constrained = limits();
    constrained.sources = 1;

    assert!(
        content
            .duplicate_source(SourceDocumentId(9), &[], constrained)
            .is_err()
    );
    assert_eq!(content, before);
}

#[test]
fn duplicate_audio_source_keeps_the_asset_reference_and_remaps_clip() {
    let asset = AudioSource {
        id: SourceId(55),
        name: "take".into(),
        path: AssetPath::external("C:/audio/take.wav"),
        frame_count: 48_000,
        sample_rate: 48_000.,
        channel_count: 2,
        byte_size: 12,
        provenance: Default::default(),
    };
    let mut content = PipelineContent::new();
    content.sources.push(SourceDocument {
        id: SourceDocumentId(9),
        content: SourceContent::Audio(vec![AudioClip::new(
            ClipId(10),
            "take",
            Ticks::ZERO,
            &asset,
        )]),
    });
    content.next_id = 56;
    let copy = content
        .duplicate_source(SourceDocumentId(9), &[55], limits())
        .expect("audio source can be copied");
    let SourceContent::Audio(clips) = &content
        .sources
        .iter()
        .find(|entry| entry.id == copy.source)
        .unwrap()
        .content
    else {
        unreachable!()
    };
    assert_eq!(clips[0].id, copy.clips[&ClipId(10)]);
    assert_eq!(clips[0].source, SourceId(55));
    assert_eq!(clips[0].name, "take");
}

#[test]
fn removing_an_in_use_source_reports_consumers_and_preserves_content() {
    let mut content = PipelineContent::new();
    content.sources.push(source());
    content.processors.push(Processor {
        id: auris_core::pipeline::NodeId(30),
        group: None,
        kind: ProcessorKind::Source(SourceDocumentId(9)),
    });
    content.next_id = 31;
    let before = content.clone();

    let error = content
        .remove_source(SourceDocumentId(9), &[], limits())
        .unwrap_err();
    assert!(matches!(
        error,
        auris_core::pipeline::content::ContentError::SourceInUse(..)
    ));
    assert_eq!(content, before);
}

#[test]
fn removing_master_is_rejected_without_lowering_high_water_mark() {
    let mut content = PipelineContent::new();
    content.next_id = 100;
    let before = content.clone();

    assert!(matches!(
        content.remove_group(content.master, &[], limits()),
        Err(auris_core::pipeline::content::ContentError::MasterRemoval)
    ));
    assert_eq!(content, before);
}

fn branch() -> PipelineContent {
    let mut content = PipelineContent::new();
    content.sources.push(source());
    let group = GroupId(21);
    let source_node = NodeId(22);
    let instrument = NodeId(12);
    let effect = NodeId(13);
    let send = NodeId(14);
    let strip = NodeId(15);
    let plugin = |name: &str| PluginInstance {
        plugin_id: name.into(),
        state: auris_core::PluginState {
            params: [("gain".into(), 0.5)].into(),
            extra: serde_json::json!({"hosted": {"bytes": [1, 2, 3]}}),
        },
        file: Some(AssetPath::external("C:/plugins/missing.clap")),
    };
    content.groups.push(ProcessingGroup {
        id: group,
        strip,
        primary_instrument: Some(instrument),
    });
    content.processors.extend([
        Processor {
            id: source_node,
            group: None,
            kind: ProcessorKind::Source(SourceDocumentId(9)),
        },
        Processor {
            id: instrument,
            group: Some(group),
            kind: ProcessorKind::Instrument(plugin("synth")),
        },
        Processor {
            id: effect,
            group: Some(group),
            kind: ProcessorKind::Effect {
                slot: auris_core::EffectSlotId(16),
                plugin: plugin("fx"),
                enabled: true,
            },
        },
        Processor {
            id: send,
            group: Some(group),
            kind: ProcessorKind::Send {
                send: auris_core::SendId(17),
                level_db: -3.,
            },
        },
        Processor {
            id: strip,
            group: Some(group),
            kind: ProcessorKind::Strip(Default::default()),
        },
    ]);
    content.edges.extend([
        PipelineEdge {
            id: EdgeId(18),
            from: (source_node, PortId::new("out")),
            to: (instrument, PortId::new("in")),
            role: EdgeRole::Events,
        },
        PipelineEdge {
            id: EdgeId(19),
            from: (instrument, PortId::new("out")),
            to: (effect, PortId::new("in")),
            role: EdgeRole::Main,
        },
        PipelineEdge {
            id: EdgeId(20),
            from: (effect, PortId::new("out")),
            to: (strip, PortId::new("in")),
            role: EdgeRole::Main,
        },
        PipelineEdge {
            id: EdgeId(23),
            from: (strip, PortId::new("out")),
            to: (NodeId(2), PortId::new("in")),
            role: EdgeRole::Main,
        },
    ]);
    content.tracks.push(TrackBinding {
        track: TrackId(24),
        source: Some(SourceDocumentId(9)),
        group,
    });
    content.next_id = 30;
    content.validate_ownership(&[], limits()).unwrap();
    content
}

#[test]
fn duplicate_branch_shares_sources_and_remaps_all_owned_connections() {
    let mut content = branch();
    let before = content.clone();
    let copy = content
        .duplicate_branch(GroupId(21), &[], limits())
        .expect("branch can be copied");
    assert_eq!(content.groups.len(), 3);
    assert_eq!(copy.nodes.len(), 4);
    assert!(!copy.nodes.contains_key(&NodeId(22)));
    assert_eq!(content.edges.len(), 8);
    assert_eq!(copy.edges.len(), 4);
    assert_eq!(copy.effects.len(), 1);
    assert_eq!(copy.sends.len(), 1);
    assert_eq!(content.tracks, before.tracks);
    assert_eq!(content.sources, before.sources);
    let copied_edge = content
        .edges
        .iter()
        .find(|edge| edge.id == copy.edges[&EdgeId(18)])
        .unwrap();
    assert_eq!(copied_edge.from.0, NodeId(22));
    assert_eq!(copied_edge.to.0, copy.nodes[&NodeId(12)]);
    for edge in &before.edges {
        let copied = content
            .edges
            .iter()
            .find(|candidate| candidate.id == copy.edges[&edge.id])
            .unwrap();
        assert_eq!(
            copied.from.0,
            *copy.nodes.get(&edge.from.0).unwrap_or(&edge.from.0)
        );
        assert_eq!(
            copied.to.0,
            *copy.nodes.get(&edge.to.0).unwrap_or(&edge.to.0)
        );
        assert_eq!(
            (&copied.from.1, &copied.to.1, copied.role),
            (&edge.from.1, &edge.to.1, edge.role)
        );
    }
    let copied_group = content
        .groups
        .iter()
        .find(|group| group.id == copy.group)
        .unwrap();
    assert_eq!(copied_group.strip, copy.nodes[&NodeId(15)]);
    assert_eq!(
        copied_group.primary_instrument,
        Some(copy.nodes[&NodeId(12)])
    );
    let copied_instrument = content
        .processors
        .iter_mut()
        .find(|node| node.id == copy.nodes[&NodeId(12)])
        .unwrap();
    let original = before
        .processors
        .iter()
        .find(|node| node.id == NodeId(12))
        .unwrap();
    assert_eq!(copied_instrument.kind, original.kind);
    let ProcessorKind::Instrument(plugin) = &mut copied_instrument.kind else {
        unreachable!()
    };
    plugin.state.params.insert("gain".into(), 0.9);
    plugin.state.extra["hosted"]["bytes"][0] = 99.into();
    assert_eq!(
        content
            .processors
            .iter()
            .find(|node| node.id == original.id)
            .unwrap(),
        original
    );
    let copied_effect = content
        .processors
        .iter()
        .find(|node| node.id == copy.nodes[&NodeId(13)])
        .unwrap();
    assert!(
        matches!(copied_effect.kind, ProcessorKind::Effect { slot, .. } if slot == copy.effects[&auris_core::EffectSlotId(16)])
    );
    let copied_send = content
        .processors
        .iter()
        .find(|node| node.id == copy.nodes[&NodeId(14)])
        .unwrap();
    assert!(
        matches!(copied_send.kind, ProcessorKind::Send { send, .. } if send == copy.sends[&auris_core::SendId(17)])
    );
    let encoded = serde_json::to_vec(&content).unwrap();
    let decoded: PipelineContent = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, content);
    decoded.validate_ownership(&[], limits()).unwrap();
}

#[test]
fn disconnected_branch_copy_does_not_invent_a_route() {
    let mut content = branch();
    content.edges.clear();
    let copy = content
        .duplicate_branch(GroupId(21), &[], limits())
        .unwrap();
    assert!(copy.edges.is_empty());
    assert!(content.edges.is_empty());
}

#[test]
fn removing_a_branch_keeps_shared_sources_and_other_branch_intact() {
    let mut content = branch();
    let copy = content
        .duplicate_branch(GroupId(21), &[], limits())
        .unwrap();
    content.tracks.push(TrackBinding {
        track: TrackId(200),
        source: Some(SourceDocumentId(9)),
        group: copy.group,
    });
    content.repair_id_counter(&[], limits()).unwrap();
    let before = content.clone();
    content.remove_group(GroupId(21), &[], limits()).unwrap();
    assert_eq!(content.sources, before.sources);
    assert_eq!(content.next_id, before.next_id);
    assert_eq!(content.tracks.len(), 1);
    assert_eq!(content.tracks[0].group, copy.group);
    assert!(
        content
            .processors
            .iter()
            .all(|node| node.group != Some(GroupId(21)))
    );
    for id in copy.nodes.values() {
        assert_eq!(
            content.processors.iter().find(|node| node.id == *id),
            before.processors.iter().find(|node| node.id == *id)
        );
    }
    assert_eq!(content.edges.len(), copy.edges.len());
    let after = content.clone();
    assert_eq!(
        content.remove_source(SourceDocumentId(9), &[], limits()),
        Err(ContentError::SourceInUse(
            SourceDocumentId(9),
            vec![
                ContentRef::Track(TrackId(200)),
                ContentRef::Node(NodeId(22))
            ]
        ))
    );
    assert_eq!(content, after);
}

#[test]
fn unused_source_removal_and_future_copy_do_not_reuse_old_ids() {
    let mut content = branch();
    let copy = content
        .duplicate_source(SourceDocumentId(9), &[], limits())
        .unwrap();
    let next = content.next_id;
    content.remove_source(copy.source, &[], limits()).unwrap();
    assert_eq!(content.next_id, next);
    let later = content
        .duplicate_source(SourceDocumentId(9), &[], limits())
        .unwrap();
    assert!(later.source.0 >= next);
    assert_ne!(copy.source, later.source);
}

#[test]
fn copy_overflow_and_limit_errors_leave_the_entire_candidate_unchanged() {
    for branch_copy in [false, true] {
        let mut content = branch();
        content.next_id = u64::MAX - 1;
        let before = content.clone();
        let error = if branch_copy {
            content
                .duplicate_branch(GroupId(21), &[], limits())
                .unwrap_err()
        } else {
            content
                .duplicate_source(SourceDocumentId(9), &[], limits())
                .unwrap_err()
        };
        assert_eq!(error, ContentError::IdExhausted);
        assert_eq!(content, before);
    }
    let mut content = branch();
    let before = content.clone();
    let mut budget = limits();
    budget.edges = content.edges.len();
    assert_eq!(
        content.duplicate_branch(GroupId(21), &[], budget),
        Err(ContentError::Limit("edges"))
    );
    assert_eq!(content, before);
    budget = limits();
    budget.stages = 1;
    assert_eq!(
        content.duplicate_source(SourceDocumentId(9), &[], budget),
        Err(ContentError::Limit("stages"))
    );
    assert_eq!(content, before);
    assert!(matches!(
        content.duplicate_branch(content.master, &[], limits()),
        Err(ContentError::Role(..))
    ));
    assert_eq!(content, before);
}
