use auris_core::PluginState;
use auris_core::automation::{AutomationCurve, AutomationPoint};
use auris_core::param::ParamUnit;
use auris_core::pipeline::content::{
    AutomationBinding, AutomationDocument, AutomationId, ContentError, ContentLimits, GroupId,
    ParameterAddress, PipelineContent, PluginInstance, ProcessingGroup, Processor, ProcessorKind,
    TrackBinding,
};
use auris_core::pipeline::{ControlDomain, EdgeId, NodeId};
use auris_core::{Ticks, TrackId};

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 8,
        clips: 16,
        stages: 32,
        processors: 32,
        groups: 8,
        tracks: 8,
        edges: 32,
        external_ids: 8,
        automations: 8,
        automation_points: 32,
        bindings: 16,
    }
}

fn curve(id: AutomationId) -> AutomationDocument {
    AutomationDocument::new(
        id,
        ControlDomain::Native {
            unit: ParamUnit::Decibels,
            min: -60.0,
            max: 12.0,
        },
        AutomationCurve::Linear,
        vec![
            AutomationPoint::new(Ticks::ZERO, -12.0),
            AutomationPoint::new(Ticks(480), 0.0),
        ],
    )
    .unwrap()
}

fn fixture() -> PipelineContent {
    let mut content = PipelineContent::new();
    content.groups.push(ProcessingGroup {
        id: GroupId(10),
        strip: NodeId(11),
        primary_instrument: None,
    });
    content.processors.extend([
        Processor {
            id: NodeId(11),
            group: Some(GroupId(10)),
            kind: ProcessorKind::Strip(Default::default()),
        },
        Processor {
            id: NodeId(12),
            group: Some(GroupId(10)),
            kind: ProcessorKind::Instrument(PluginInstance {
                plugin_id: "instrument".into(),
                state: PluginState::default(),
                file: None,
            }),
        },
        Processor {
            id: NodeId(20),
            group: None,
            kind: ProcessorKind::Automation(AutomationId(30)),
        },
    ]);
    content.tracks.push(TrackBinding {
        track: TrackId(40),
        source: None,
        group: GroupId(10),
    });
    content.automation.push(curve(AutomationId(30)));
    content.automation_bindings.extend([
        AutomationBinding {
            id: EdgeId(50),
            source: NodeId(20),
            target: ParameterAddress {
                node: NodeId(11),
                key: "gain_db".into(),
            },
            enabled: true,
        },
        AutomationBinding {
            id: EdgeId(51),
            source: NodeId(20),
            target: ParameterAddress {
                node: NodeId(12),
                key: "gain_db".into(),
            },
            enabled: false,
        },
    ]);
    content.next_id = 100;
    content
}

#[test]
fn one_curve_can_feed_multiple_bindings_with_different_enabled_states() {
    let content = fixture();
    content.validate_ownership(&[], limits()).unwrap();
    assert_eq!(content.automation.len(), 1);
    assert_eq!(content.automation_bindings.len(), 2);
    assert_eq!(
        content.automation_bindings[0].source,
        content.automation_bindings[1].source
    );
    assert!(content.automation_bindings[0].enabled);
    assert!(!content.automation_bindings[1].enabled);
}

#[test]
fn automation_and_binding_ids_share_the_global_namespace() {
    let mut content = fixture();
    content.automation[0].id = AutomationId(11);
    assert_eq!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::DuplicateId(11))
    );

    let mut content = fixture();
    content.automation_bindings[1].id = EdgeId(30);
    assert_eq!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::DuplicateId(30))
    );
}

#[test]
fn counter_repair_accounts_for_automation_objects_and_external_ids() {
    let mut content = fixture();
    content.next_id = 1;
    content.repair_id_counter(&[900], limits()).unwrap();
    assert_eq!(content.next_id, 901);
    assert!(content.validate_ownership(&[900], limits()).is_ok());
}

#[test]
fn automation_references_must_name_existing_source_and_parameter_nodes() {
    let mut content = fixture();
    content.automation_bindings[0].source = NodeId(999);
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Missing(_))
    ));

    let mut content = fixture();
    content.automation_bindings[0].target.node = NodeId(999);
    assert!(matches!(
        content.validate_ownership(&[], limits()),
        Err(ContentError::Missing(_))
    ));
}

#[test]
fn automation_round_trip_preserves_shared_curve_and_disabled_binding() {
    let content = fixture();
    let encoded = serde_json::to_vec(&content).unwrap();
    let decoded: PipelineContent = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, content);
    assert_eq!(decoded.automation[0].value_at(Ticks(240)), -6.0);
    assert!(!decoded.automation_bindings[1].enabled);
}

#[test]
fn duplicate_automation_allocates_a_new_curve_id_and_preserves_points() {
    let mut content = fixture();
    let copy = content
        .duplicate_automation(AutomationId(30), &[], limits())
        .unwrap();
    assert_ne!(copy, AutomationId(30));
    let original = content
        .automation
        .iter()
        .find(|curve| curve.id == AutomationId(30))
        .unwrap();
    let duplicated = content
        .automation
        .iter()
        .find(|curve| curve.id == copy)
        .unwrap();
    assert_eq!(duplicated.points(), original.points());
    assert_eq!(duplicated.curve, original.curve);
    assert_eq!(duplicated.domain, original.domain);
}

#[test]
fn automation_removal_is_blocked_while_its_source_processor_exists() {
    let mut content = fixture();
    let before = content.clone();
    assert!(matches!(
        content.remove_automation(AutomationId(30), &[], limits()),
        Err(ContentError::AutomationInUse(..))
    ));
    assert_eq!(content, before);
}

#[test]
fn branch_copy_remaps_owned_automation_bindings_but_shares_the_curve() {
    let mut content = fixture();
    let copy = content
        .duplicate_branch(GroupId(10), &[], limits())
        .unwrap();
    assert_eq!(copy.bindings.len(), 2);
    assert!(
        copy.bindings
            .values()
            .all(|id| *id != EdgeId(50) && *id != EdgeId(51))
    );
    assert_eq!(content.automation.len(), 1);
    for (original_id, binding_id) in &copy.bindings {
        let binding = content
            .automation_bindings
            .iter()
            .find(|binding| binding.id == *binding_id)
            .unwrap();
        assert_eq!(binding.source, NodeId(20));
        let original = content
            .automation_bindings
            .iter()
            .find(|binding| binding.id == *original_id)
            .unwrap();
        assert_eq!(binding.target.node, copy.nodes[&original.target.node]);
        assert_eq!(binding.target.key, original.target.key);
        assert_eq!(binding.enabled, original.enabled);
    }
}

#[test]
fn group_removal_removes_bindings_but_keeps_shared_curve() {
    let mut content = fixture();
    content.remove_group(GroupId(10), &[], limits()).unwrap();
    assert_eq!(content.automation.len(), 1);
    assert!(content.automation_bindings.is_empty());
}

#[test]
fn source_duplication_does_not_copy_project_tick_automation() {
    let mut content = fixture();
    let before = content.automation_bindings.clone();
    content
        .duplicate_source(
            auris_core::pipeline::content::SourceDocumentId(999),
            &[],
            limits(),
        )
        .unwrap_err();
    assert_eq!(content.automation_bindings, before);
}
