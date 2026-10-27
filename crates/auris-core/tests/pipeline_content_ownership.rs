use auris_core::pipeline::content::{
    ContentError, ContentLimits, ContentRef, GroupId, PipelineContent, ProcessingGroup, Processor,
    ProcessorKind, SourceContent, SourceDocument, SourceDocumentId, StripState, TrackBinding,
};
use auris_core::pipeline::{EdgeId, EdgeRole, NodeId, PipelineEdge, PortId};
use auris_core::project::{ClipId, MidiClip, NoteTransform, StageId};
use auris_core::{Ticks, TrackId};

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 8,
        clips: 32,
        stages: 64,
        processors: 32,
        groups: 8,
        tracks: 16,
        edges: 32,
        external_ids: 4,
        automations: 0,
        automation_points: 0,
        bindings: 0,
    }
}

fn valid() -> PipelineContent {
    let mut content = PipelineContent::new();
    let mut clip = MidiClip::new(ClipId(20), "clip", Ticks::ZERO, Ticks(480));
    clip.transforms
        .push(NoteTransform::Transpose { semitones: 12 });
    content.sources.push(SourceDocument {
        id: SourceDocumentId(10),
        content: SourceContent::Notes(vec![clip.with_stage_ids(vec![StageId(30)]).unwrap()]),
    });
    content.groups.push(ProcessingGroup {
        id: GroupId(40),
        strip: NodeId(41),
        primary_instrument: None,
    });
    content.processors.push(Processor {
        id: NodeId(41),
        group: Some(GroupId(40)),
        kind: ProcessorKind::Strip(StripState::default()),
    });
    content.tracks.push(TrackBinding {
        track: TrackId(50),
        source: Some(SourceDocumentId(10)),
        group: GroupId(40),
    });
    content.next_id = 100;
    content
}

#[test]
fn every_owner_uses_the_shared_numeric_namespace() {
    let mut content = valid();
    content.groups[1].id = GroupId(10); // source ID
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::DuplicateId(10))
    ));
}

#[test]
fn dangling_references_and_invalid_roles_are_rejected() {
    let mut content = valid();
    content.tracks[0].source = Some(SourceDocumentId(999));
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Missing(_))
    ));

    let mut content = valid();
    content.processors[1].group = None;
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Role(ContentRef::Node(_), _))
    ));
}

#[test]
fn source_documents_may_be_shared_but_tracks_cannot_use_master_group() {
    let mut content = valid();
    content.tracks.push(TrackBinding {
        track: TrackId(51),
        source: Some(SourceDocumentId(10)),
        group: GroupId(40),
    });
    assert!(content.validate_ownership(&[], limits()).is_ok());
    content.tracks[0].group = content.master;
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Role(..))
    ));
}

#[test]
fn repair_preserves_high_water_mark_after_deletion() {
    let mut content = valid();
    content.next_id = 500;
    content.sources.clear();
    content.tracks[0].source = None;
    content.repair_id_counter(&[], limits()).unwrap();
    assert_eq!(content.next_id, 500);
    assert!(content.validate_ownership(&[], limits()).is_ok());
}

#[test]
fn external_ids_participate_in_duplicates_limits_and_counter_repair() {
    let content = valid();
    assert!(matches!(
        content.validate_ownership(&[10], limits()),
        Err(ContentError::DuplicateId(10))
    ));
    let mut constrained = limits();
    constrained.external_ids = 0;
    assert!(matches!(
        content.validate_ownership(&[900], constrained),
        Err(ContentError::Limit("external_ids"))
    ));
    let mut repaired = content;
    repaired.next_id = 1;
    repaired.repair_id_counter(&[900], limits()).unwrap();
    assert_eq!(repaired.next_id, 901);
}

#[test]
fn counter_overflow_is_atomic() {
    let content = valid();
    let mut overflow = content.clone();
    overflow.next_id = 7;
    let before = overflow.clone();
    assert!(matches!(
        overflow.repair_id_counter(&[u64::MAX], limits()),
        Err(ContentError::IdExhausted)
    ));
    assert_eq!(overflow, before);
}

#[test]
fn stage_ids_collide_across_clips_and_with_external_owners() {
    let mut content = valid();
    let SourceContent::Notes(clips) = &mut content.sources[0].content else {
        unreachable!()
    };
    let mut duplicate = clips[0].clone();
    duplicate.id = ClipId(21);
    clips.push(duplicate);
    assert_eq!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::DuplicateId(30))
    );
    let content = valid();
    assert_eq!(
        content.validate_ownership(&[30], limits()),
        Err(ContentError::DuplicateId(30))
    );
}

#[test]
fn every_identity_collection_is_count_bounded_before_reference_validation() {
    let mut content = valid();
    content.edges.push(PipelineEdge {
        id: EdgeId(60),
        from: (NodeId(41), PortId::new("out")),
        to: (NodeId(2), PortId::new("in")),
        role: EdgeRole::Main,
    });
    content.validate_ownership(&[90], limits()).unwrap();
    let counts = [1, 1, 1, 2, 2, 1, 1, 1];
    let names = [
        "sources",
        "clips",
        "stages",
        "processors",
        "groups",
        "tracks",
        "edges",
        "external_ids",
    ];
    let exact = ContentLimits {
        sources: 1,
        clips: 1,
        stages: 1,
        processors: 2,
        groups: 2,
        tracks: 1,
        edges: 1,
        external_ids: 1,
        automations: 0,
        automation_points: 0,
        bindings: 0,
    };
    content.validate_ownership(&[90], exact).unwrap();
    // Reference faults must not bypass the inexpensive count preflight.
    content.tracks[0].source = Some(SourceDocumentId(999));
    for (index, count) in counts.into_iter().enumerate() {
        let mut budget = exact;
        let fields = [
            &mut budget.sources,
            &mut budget.clips,
            &mut budget.stages,
            &mut budget.processors,
            &mut budget.groups,
            &mut budget.tracks,
            &mut budget.edges,
            &mut budget.external_ids,
        ];
        *fields.into_iter().nth(index).unwrap() = count - 1;
        assert_eq!(
            content.validate_ownership(&[90], budget),
            Err(ContentError::Limit(names[index]))
        );
    }
}

#[test]
fn owner_roles_and_edge_endpoints_are_checked_without_guessing() {
    let mut content = valid();
    content.processors.push(Processor {
        id: NodeId(60),
        group: Some(GroupId(40)),
        kind: ProcessorKind::Source(SourceDocumentId(10)),
    });
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Role(ContentRef::Node(NodeId(60)), _))
    ));
    content.processors[2].group = None;
    content.validate_ownership(&[], limits()).unwrap();
    content.groups[1].primary_instrument = Some(NodeId(60));
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Role(ContentRef::Group(GroupId(40)), _))
    ));

    let mut content = valid();
    content.groups[1].strip = NodeId(2);
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Role(..))
    ));
    let mut content = valid();
    content.edges.push(PipelineEdge {
        id: EdgeId(60),
        from: (NodeId(41), PortId::new("out")),
        to: (NodeId(999), PortId::new("in")),
        role: EdgeRole::Main,
    });
    assert_eq!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Missing(ContentRef::Node(NodeId(999))))
    );
}

#[test]
fn counter_repair_refuses_invalid_owners_without_partial_mutation() {
    let mut content = valid();
    content.next_id = 1;
    assert_eq!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::StaleCounter(50))
    );
    content.sources[0].id = SourceDocumentId(40);
    let before = content.clone();
    assert_eq!(
        content.repair_id_counter(&[], limits()),
        Err(ContentError::DuplicateId(40))
    );
    assert_eq!(content, before);
}
