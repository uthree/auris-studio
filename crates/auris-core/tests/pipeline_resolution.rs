use auris_core::automation::{AutomationCurve, AutomationPoint};
use auris_core::pipeline::content::*;
use auris_core::pipeline::{
    ControlDomain, EdgeId, EdgeRole, NodeId, PipelineEdge, PipelineError, PipelineLimits, PortId,
};
use auris_core::{ParamUnit, Ticks};

struct NoPlugins;
impl CapabilityProvider for NoPlugins {
    fn plugin<'a>(&'a self, _: &Processor) -> Option<PluginCapabilities<'a>> {
        None
    }
}

fn limits() -> ContentLimits {
    ContentLimits {
        sources: 16,
        clips: 16,
        stages: 16,
        processors: 16,
        groups: 16,
        tracks: 16,
        edges: 16,
        external_ids: 16,
        automations: 16,
        automation_points: 16,
        bindings: 16,
    }
}
fn graph_limits() -> PipelineLimits {
    PipelineLimits {
        nodes: 16,
        edges: 32,
        ports: 64,
        parameter_keys: 16,
        text_bytes: 4096,
    }
}
fn fixture() -> PipelineContent {
    let mut content = PipelineContent::new();
    content.groups.push(ProcessingGroup {
        id: GroupId(3),
        strip: NodeId(4),
        primary_instrument: None,
    });
    content.processors.push(Processor {
        id: NodeId(4),
        group: Some(GroupId(3)),
        kind: ProcessorKind::Strip(StripState {
            gain_db: -3.0,
            ..Default::default()
        }),
    });
    content.automation.push(
        AutomationDocument::new(
            AutomationId(5),
            ControlDomain::Native {
                unit: ParamUnit::Decibels,
                min: -60.0,
                max: 12.0,
            },
            AutomationCurve::Linear,
            vec![
                AutomationPoint::new(Ticks(10), -12.0),
                AutomationPoint::new(Ticks(110), 0.0),
            ],
        )
        .unwrap(),
    );
    content.processors.push(Processor {
        id: NodeId(6),
        group: None,
        kind: ProcessorKind::Automation(AutomationId(5)),
    });
    for (id, node) in [(7, 2), (8, 4)] {
        content.automation_bindings.push(AutomationBinding {
            id: EdgeId(id),
            source: NodeId(6),
            target: ParameterAddress {
                node: NodeId(node),
                key: "gain".into(),
            },
            enabled: true,
        });
    }
    content.next_id = 9;
    content
}

#[test]
fn unresolved_curve_is_preserved_until_a_compatible_domain_is_supplied() {
    let mut content = fixture();
    content.automation[0].domain = None;
    let before = content.clone();
    assert!(matches!(
        content.resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins),
        Err(ResolutionError::Capability(
            CapabilityError::UnresolvedAutomation(AutomationId(5))
        ))
    ));
    assert_eq!(content, before);
    content.automation[0]
        .resolve_domain(ControlDomain::Native {
            unit: ParamUnit::Decibels,
            min: -60.0,
            max: 12.0,
        })
        .unwrap();
    content
        .resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins)
        .unwrap();
}

#[test]
fn shared_curve_resolves_once_and_drives_two_stable_targets() {
    let content = fixture();
    let before = content.clone();
    let resolved = content
        .resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins)
        .unwrap();
    assert_eq!(resolved.pipeline().edges.len(), 2);
    assert_eq!(
        resolved.topological_order(),
        &[NodeId(6), NodeId(2), NodeId(4)]
    );
    for node in [NodeId(2), NodeId(4)] {
        let address = ParameterAddress {
            node,
            key: "gain".into(),
        };
        assert_eq!(
            resolved.automation_value(&address, Ticks::ZERO),
            Some(-12.0)
        );
        assert_eq!(resolved.automation_value(&address, Ticks(60)), Some(-6.0));
        assert_eq!(resolved.automation_value(&address, Ticks(1000)), Some(0.0));
    }
    assert_eq!(content, before);
}

#[test]
fn disabled_binding_returns_static_value_without_mutating_curve_or_strip() {
    let mut content = fixture();
    content.automation_bindings[1].enabled = false;
    let resolved = content
        .resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins)
        .unwrap();
    assert_eq!(resolved.pipeline().edges.len(), 1);
    let address = ParameterAddress {
        node: NodeId(4),
        key: "gain".into(),
    };
    assert_eq!(resolved.automation_value(&address, Ticks(60)), None);
    assert!(matches!(
        content.processors[1].kind,
        ProcessorKind::Strip(StripState { gain_db: -3.0, .. })
    ));
    assert_eq!(content.automation[0].points().len(), 2);
}

#[test]
fn only_enabled_connections_compete_for_the_absolute_driver() {
    let mut content = fixture();
    let mut second = content.automation_bindings[0].clone();
    second.id = EdgeId(9);
    content.next_id = 10;
    second.enabled = false;
    content.automation_bindings.push(second);
    content
        .resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins)
        .unwrap();
    content.automation_bindings[2].enabled = true;
    assert!(matches!(
        content.resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins),
        Err(ResolutionError::Topology(
            PipelineError::ConflictingDrivers { .. }
        ))
    ));
}

#[test]
fn disabled_bindings_still_require_valid_targets_and_exact_domains() {
    let mut content = fixture();
    content.automation_bindings[0].enabled = false;
    content.automation_bindings[0].target.key = "missing".into();
    assert!(matches!(
        content.resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins),
        Err(ResolutionError::Topology(
            PipelineError::UnknownParameter { .. }
        ))
    ));
    content.automation_bindings[0].target.key = "pan".into();
    assert_eq!(
        content
            .resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins)
            .unwrap_err(),
        ResolutionError::BindingDomain(EdgeId(7))
    );
}

#[test]
fn control_connections_cannot_bypass_binding_ownership() {
    let mut content = fixture();
    content.edges.push(PipelineEdge {
        id: EdgeId(9),
        from: (NodeId(6), PortId::new("control_out")),
        to: (NodeId(2), PortId::new("param:gain")),
        role: EdgeRole::Control,
    });
    content.next_id = 10;
    assert!(matches!(
        content.resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins),
        Err(ResolutionError::Content(ContentError::Role(..)))
    ));
}

#[test]
fn audio_cycles_remain_invalid_after_control_bindings_are_resolved() {
    let mut content = fixture();
    for (id, from, to) in [(9, 2, 4), (10, 4, 2)] {
        content.edges.push(PipelineEdge {
            id: EdgeId(id),
            from: (NodeId(from), PortId::new("audio_out")),
            to: (NodeId(to), PortId::new("audio_in")),
            role: EdgeRole::Main,
        });
    }
    content.next_id = 11;
    assert!(matches!(
        content.resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins),
        Err(ResolutionError::Topology(PipelineError::Cycle { .. }))
    ));
}

#[test]
fn resolution_budgets_include_disabled_bindings_and_cumulative_node_text() {
    let mut content = fixture();
    content
        .automation_bindings
        .iter_mut()
        .for_each(|binding| binding.enabled = false);
    let mut graph = graph_limits();
    graph.edges = 1;
    assert_eq!(
        content
            .resolve_pipeline(&[], limits(), graph, &NoPlugins)
            .unwrap_err(),
        ResolutionError::Limit("edges")
    );
    graph = graph_limits();
    graph.text_bytes = 30;
    assert_eq!(
        content
            .resolve_pipeline(&[], limits(), graph, &NoPlugins)
            .unwrap_err(),
        ResolutionError::Limit("text_bytes")
    );
    graph = graph_limits();
    graph.ports = 5;
    assert!(matches!(
        content.resolve_pipeline(&[], limits(), graph, &NoPlugins),
        Err(ResolutionError::Capability(CapabilityError::Limit("ports")))
    ));
}

#[test]
fn unresolved_plugin_metadata_preserves_the_saved_candidate() {
    let mut content = fixture();
    content.processors.push(Processor {
        id: NodeId(9),
        group: Some(GroupId(3)),
        kind: ProcessorKind::Instrument(PluginInstance {
            plugin_id: "missing".into(),
            state: Default::default(),
            file: None,
        }),
    });
    content.next_id = 10;
    let before = content.clone();
    assert!(matches!(
        content.resolve_pipeline(&[], limits(), graph_limits(), &NoPlugins),
        Err(ResolutionError::Capability(
            CapabilityError::MissingPlugin { node: NodeId(9) }
        ))
    ));
    assert_eq!(content, before);
}
