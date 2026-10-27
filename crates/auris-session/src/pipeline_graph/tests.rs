use super::*;
use auris_core::project::{AuxSend, SingerTake};
use auris_core::{AssetPath, Note, NoteTransform, ParamId, Ticks};

fn fixture() -> Project {
    let mut project = Project::new("Projection", 48_000.0);
    let track = project.add_instrument_track("Lead", "auris.synth.chiptune");
    let bus = project.add_bus_track("Bus");
    let clip = project
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
        .unwrap();
    let midi = project.midi_clip_mut(clip).unwrap();
    midi.notes.push(Note::new(60, Ticks::ZERO, Ticks::QUARTER));
    midi.transforms = vec![
        NoteTransform::Gate { amount: 0.5 },
        NoteTransform::Gate { amount: 0.5 },
    ];
    project.add_effect(Some(track), "auris.fx.limiter").unwrap();
    let detector = project
        .add_effect(Some(bus), "auris.fx.compressor")
        .unwrap();
    project
        .track_mut(bus)
        .unwrap()
        .mixer
        .effects
        .iter_mut()
        .find(|slot| slot.id == detector)
        .unwrap()
        .sidechain = Some(track);
    for pre_fader in [true, false] {
        let id = project.next_send_id();
        project.track_mut(track).unwrap().sends.push(AuxSend {
            id,
            target: bus,
            level_db: -6.0,
            pre_fader,
        });
        project.automation.set_point(
            ParamTarget::Send { track, send: id },
            None,
            AutomationCurve::Linear,
            Ticks::ZERO,
            -3.0,
        );
    }
    project
}

fn has_edge(graph: &Projection, source: &str, target: &str, kind: PipelineEdgeKind) -> bool {
    graph
        .edges
        .iter()
        .any(|edge| edge.source == source && edge.target == target && edge.kind == kind)
}

fn assert_closed(graph: &Projection) {
    let ids: HashSet<_> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(ids.len(), graph.nodes.len(), "duplicate node identities");
    for edge in &graph.edges {
        assert!(
            ids.contains(edge.source.as_str()),
            "missing source {}",
            edge.source
        );
        assert!(
            ids.contains(edge.target.as_str()),
            "missing target {}",
            edge.target
        );
        let source = graph
            .nodes
            .iter()
            .find(|node| node.id == edge.source)
            .unwrap();
        let target = graph
            .nodes
            .iter()
            .find(|node| node.id == edge.target)
            .unwrap();
        assert!(source.outputs.contains(&edge.domain));
        assert!(target.inputs.contains(&edge.domain));
    }
}

#[test]
fn routing_distinguishes_post_insert_taps_faders_sidechains_and_send_controls() {
    let project = fixture();
    let before = project.clone();
    let graph = Projection::from_project(&project).unwrap();
    let track = &project.tracks[0];
    let bus = &project.tracks[1];
    let input = strip(Some(track.id));
    let effect = format!("effect:{}", track.mixer.effects[0].id.0);
    let post = fader(Some(track.id));
    assert!(has_edge(
        &graph,
        &instrument(track.id),
        &input,
        PipelineEdgeKind::Playback
    ));
    assert!(has_edge(&graph, &input, &effect, PipelineEdgeKind::Chain));
    assert!(has_edge(&graph, &effect, &post, PipelineEdgeKind::Chain));
    assert!(has_edge(
        &graph,
        &post,
        &strip(None),
        PipelineEdgeKind::Output
    ));
    assert!(has_edge(
        &graph,
        &post,
        &format!("effect:{}", bus.mixer.effects[0].id.0),
        PipelineEdgeKind::Sidechain
    ));
    for send in &track.sends {
        let id = format!("send:{}", send.id.0);
        let tap = if send.pre_fader { &effect } else { &post };
        assert!(has_edge(&graph, tap, &id, PipelineEdgeKind::Send));
        assert!(has_edge(
            &graph,
            &id,
            &strip(Some(bus.id)),
            PipelineEdgeKind::Send
        ));
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.target == id && edge.kind == PipelineEdgeKind::Automation)
        );
        assert!(graph.nodes.iter().any(|node| node.id == id && matches!(node.data, PipelineNodeData::Send { pre_fader, level_db: -6.0, .. } if pre_fader == send.pre_fader)));
    }
    assert_closed(&graph);
    assert_eq!(project, before);
}

#[test]
fn repeated_stages_have_distinct_revision_scoped_paths_and_preserve_authored_notes() {
    let project = fixture();
    let clip = &project.tracks[0].kind.note_clips().unwrap()[0];
    let before = clip.notes.clone();
    let graph = Projection::from_project(&project).unwrap();
    let stages: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| matches!(node.data, PipelineNodeData::ClipStage { .. }))
        .collect();
    assert_eq!(stages.len(), 2);
    assert_ne!(stages[0].id, stages[1].id);
    assert!(stages.iter().all(|node| node.revision_scoped));
    let fingerprints: Vec<_> = stages
        .iter()
        .map(|node| match &node.data {
            PipelineNodeData::ClipStage { fingerprint, .. } => fingerprint,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(fingerprints[0], fingerprints[1]);
    assert!(has_edge(
        &graph,
        &format!("source:clip:{}", clip.id.0),
        &format!("clip:{}", clip.id.0),
        PipelineEdgeKind::Content
    ));
    assert!(has_edge(
        &graph,
        &stages[0].id,
        &stages[1].id,
        PipelineEdgeKind::Performance
    ));
    assert_eq!(project.midi_clip(clip.id).unwrap().1.notes, before);
}

#[test]
fn generated_curve_stages_and_composite_stacks_keep_their_evaluation_phase() {
    let mut project = fixture();
    let clip = project.tracks[0].kind.note_clips().unwrap()[0].id;
    project.midi_clip_mut(clip).unwrap().transforms = vec![
        NoteTransform::Pitch {
            settings: Default::default(),
        },
        NoteTransform::Gate { amount: 0.5 },
        NoteTransform::ForDrumVoice {
            voice: "kick".into(),
            transforms: vec![NoteTransform::Gate { amount: 0.5 }],
        },
    ];
    let graph = Projection::from_project(&project).unwrap();
    let phases: Vec<_> = graph
        .nodes
        .iter()
        .filter_map(|node| match node.data {
            PipelineNodeData::ClipStage { evaluation, .. } => Some(evaluation),
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        vec![
            PipelineStageEvaluation::AfterNoteStages,
            PipelineStageEvaluation::PerPassNotes,
            PipelineStageEvaluation::Composite
        ]
    );
}

#[test]
fn focused_reads_include_boundary_neighbors_and_reject_missing_objects() {
    let project = fixture();
    let track = project.tracks[0].id;
    let clip = project.tracks[0].kind.note_clips().unwrap()[0].id;
    for scope in [
        PipelineScope::Track { track: track.0 },
        PipelineScope::Clip { clip: clip.0 },
        PipelineScope::Node {
            node: fader(Some(track)),
        },
    ] {
        let mut graph = Projection::from_project(&project).unwrap();
        graph.focus(&scope).unwrap();
        assert!(!graph.edges.is_empty());
        assert_closed(&graph);
    }
    let mut graph = Projection::from_project(&project).unwrap();
    assert!(matches!(
        graph.focus(&PipelineScope::Track { track: u64::MAX }),
        Err(PipelineError::UnknownScope)
    ));
}

#[test]
fn independent_streams_remain_bounded_complete_and_revision_consistent() {
    let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
    for index in 0..70 {
        session.add_bus_track(format!("Bus {index}"));
    }
    let first = session
        .pipeline_graph(&PipelineQuery {
            limit: Some(usize::MAX),
            edge_limit: Some(usize::MAX),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(first.nodes.len(), MAX_NODES);
    assert_eq!(first.edges.len(), MAX_EDGES);
    let revision = first.revision.clone();
    let mut graph = Projection {
        nodes: first.nodes,
        edges: first.edges,
    };
    let mut offset = first.next_offset.unwrap_or(first.total_nodes);
    let mut edge_offset = first.next_edge_offset.unwrap_or(first.total_edges);
    while offset < first.total_nodes || edge_offset < first.total_edges {
        let page = session
            .pipeline_graph(&PipelineQuery {
                offset,
                edge_offset,
                expected_revision: Some(revision.clone()),
                ..Default::default()
            })
            .unwrap();
        offset = page.next_offset.unwrap_or(page.total_nodes);
        edge_offset = page.next_edge_offset.unwrap_or(page.total_edges);
        assert_eq!(page.revision, revision);
        graph.nodes.extend(page.nodes);
        graph.edges.extend(page.edges);
    }
    assert_eq!(graph.nodes.len(), first.total_nodes);
    assert_eq!(graph.edges.len(), first.total_edges);
    assert_closed(&graph);
    for query in [
        PipelineQuery {
            offset: 1,
            ..Default::default()
        },
        PipelineQuery {
            edge_offset: 1,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            session.pipeline_graph(&query),
            Err(PipelineError::RevisionRequired)
        ));
    }
    session.add_bus_track("Changed");
    assert!(matches!(
        session.pipeline_graph(&PipelineQuery {
            expected_revision: Some(revision),
            ..Default::default()
        }),
        Err(PipelineError::RevisionMismatch { .. })
    ));
}

#[test]
fn singer_takes_keep_offline_dependencies_and_do_not_claim_freshness() {
    let mut project = Project::new("Singer", 48_000.0);
    let track = project.add_singer_track("Vocal", "auris.synth.chiptune");
    project
        .add_midi_clip(track, "Words", Ticks::ZERO, Ticks::QUARTER)
        .unwrap();
    let absent = Projection::from_project(&project).unwrap();
    assert!(has_edge(
        &absent,
        &instrument(track),
        &strip(Some(track)),
        PipelineEdgeKind::Playback
    ));
    let source = project.add_audio_source(
        "Take",
        AssetPath::inside("Audio/missing.wav"),
        480,
        48_000.0,
        2,
    );
    project
        .track_mut(track)
        .unwrap()
        .kind
        .as_singer_mut()
        .unwrap()
        .take = Some(SingerTake {
        provenance: None,
        source,
        fingerprint: 7,
        seed: 3,
        backend_pitch: None,
    });
    let with_take = Projection::from_project(&project).unwrap();
    assert!(with_take.nodes.iter().any(|node| matches!(
        node.data,
        PipelineNodeData::SingerTake {
            freshness_checked: false,
            ..
        }
    )));
    assert!(!has_edge(
        &with_take,
        &instrument(track),
        &strip(Some(track)),
        PipelineEdgeKind::Playback
    ));
    assert!(with_take.nodes.iter().any(|node| matches!(
        node.data,
        PipelineNodeData::Instrument {
            audition_only: true,
            ..
        }
    )));
    assert!(
        with_take
            .edges
            .iter()
            .any(|edge| edge.kind == PipelineEdgeKind::SynthesisDependency)
    );
    assert_closed(&with_take);
    project.audio_sources.remove(&source);
    let missing = Projection::from_project(&project).unwrap();
    assert!(has_edge(
        &missing,
        &instrument(track),
        &strip(Some(track)),
        PipelineEdgeKind::Playback
    ));
    assert!(missing.nodes.iter().any(|node| matches!(
        node.data,
        PipelineNodeData::AudioSource {
            registered: false,
            ..
        }
    )));
    assert_closed(&missing);
}

#[test]
fn plugin_binding_keys_are_visible_but_unchecked_and_wrong_owners_are_not_connected() {
    let mut project = fixture();
    let track = project.tracks[0].id;
    let bus = project.tracks[1].id;
    let slot = project.tracks[0].mixer.effects[0].id;
    project.automation.set_point(
        ParamTarget::Effect {
            track: Some(track),
            slot,
            param: ParamId(999),
        },
        Some("missing-key".into()),
        AutomationCurve::Hold,
        Ticks::ZERO,
        0.5,
    );
    project.automation.set_point(
        ParamTarget::Effect {
            track: Some(bus),
            slot,
            param: ParamId(998),
        },
        Some("wrong-owner".into()),
        AutomationCurve::Hold,
        Ticks::ZERO,
        0.5,
    );
    let graph = Projection::from_project(&project).unwrap();
    for node in &graph.nodes {
        if let PipelineNodeData::Automation {
            key: Some(key),
            owner_present,
            parameter_checked,
            value_unit,
            ..
        } = &node.data
        {
            assert!(!parameter_checked);
            assert!(value_unit.is_none());
            assert!(node.revision_scoped);
            assert_eq!(*owner_present, key == "missing-key");
            assert_eq!(
                graph.edges.iter().any(|edge| edge.source == node.id),
                *owner_present
            );
        }
    }
    assert_closed(&graph);
}
