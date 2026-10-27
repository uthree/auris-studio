use auris_core::ParamUnit;
use auris_core::pipeline::{
    AudioLayout, AutomationPolicy, ControlDomain, EdgeId, EdgeRole, NodeId, Parameter, Pipeline,
    PipelineEdge, PipelineError, PipelineLimits, PipelineNode, Port, PortId, PortType,
};

fn limits() -> PipelineLimits {
    PipelineLimits {
        nodes: 128,
        edges: 256,
        ports: 512,
        parameter_keys: 128,
        text_bytes: 16_384,
    }
}

fn edge(
    id: u64,
    from: u64,
    from_port: &str,
    to: u64,
    to_port: &str,
    role: EdgeRole,
) -> PipelineEdge {
    PipelineEdge {
        id: EdgeId(id),
        from: (NodeId(from), PortId::new(from_port)),
        to: (NodeId(to), PortId::new(to_port)),
        role,
    }
}

#[test]
fn shared_events_can_feed_independent_instruments_and_an_explicit_audio_sum() {
    let events = PipelineNode::new(NodeId(1)).output(Port::new("events", PortType::Events));
    let instrument_a = PipelineNode::new(NodeId(2))
        .input(Port::new("events", PortType::Events))
        .output(Port::new("audio", PortType::Audio(AudioLayout::Stereo)));
    let instrument_b = PipelineNode::new(NodeId(3))
        .input(Port::new("events", PortType::Events))
        .output(Port::new("audio", PortType::Audio(AudioLayout::Stereo)));
    let sum = PipelineNode::new(NodeId(4))
        .input(Port::sum_input("audio_in", AudioLayout::Stereo))
        .output(Port::new("audio", PortType::Audio(AudioLayout::Stereo)));
    let graph = Pipeline {
        nodes: vec![sum, instrument_b, events, instrument_a],
        edges: vec![
            edge(11, 1, "events", 2, "events", EdgeRole::Events),
            edge(12, 1, "events", 3, "events", EdgeRole::Events),
            edge(13, 2, "audio", 4, "audio_in", EdgeRole::Main),
            edge(14, 3, "audio", 4, "audio_in", EdgeRole::Main),
        ],
    };

    let result = graph
        .validate(limits())
        .expect("layered audio graph is valid");
    assert_eq!(
        result.topological_order,
        vec![NodeId(1), NodeId(2), NodeId(3), NodeId(4)]
    );
}

#[test]
fn sidechain_edges_are_audio_but_still_cannot_form_a_cycle() {
    let a = PipelineNode::new(NodeId(1))
        .input(Port::sidechain_input("key", AudioLayout::Stereo))
        .output(Port::new("main", PortType::Audio(AudioLayout::Stereo)));
    let b = PipelineNode::new(NodeId(2))
        .input(Port::sidechain_input("key", AudioLayout::Stereo))
        .output(Port::new("main", PortType::Audio(AudioLayout::Stereo)));
    let graph = Pipeline {
        nodes: vec![a, b],
        edges: vec![
            edge(21, 1, "main", 2, "key", EdgeRole::Sidechain),
            edge(22, 2, "main", 1, "key", EdgeRole::Sidechain),
        ],
    };

    let Err(PipelineError::Cycle { nodes, edges }) = graph.validate(limits()) else {
        panic!("sidechain feedback must be rejected");
    };
    assert_eq!(nodes.first(), nodes.last());
    assert_eq!(edges, vec![EdgeId(21), EdgeId(22)]);
}

#[test]
fn control_domains_require_the_same_unit_and_finite_range() {
    let source = PipelineNode::new(NodeId(1)).output(Port::new(
        "cutoff",
        PortType::Control(ControlDomain::Normalized { min: 0.0, max: 1.0 }),
    ));
    let target = PipelineNode::new(NodeId(2)).input(Port::new(
        "cutoff",
        PortType::Control(ControlDomain::Native {
            unit: ParamUnit::Hertz,
            min: 20.0,
            max: 20_000.0,
        }),
    ));
    let graph = Pipeline {
        nodes: vec![source, target],
        edges: vec![edge(31, 1, "cutoff", 2, "cutoff", EdgeRole::Control)],
    };
    assert_eq!(
        graph.validate(limits()),
        Err(PipelineError::TypeMismatch { edge: EdgeId(31) })
    );

    let invalid = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(3)).input(Port::new(
            "bad",
            PortType::Control(ControlDomain::Native {
                unit: ParamUnit::Hertz,
                min: f32::NAN,
                max: 20_000.0,
            }),
        ))],
        edges: vec![],
    };
    assert!(matches!(
        invalid.validate(limits()),
        Err(PipelineError::InvalidPortRange { .. })
    ));
}

#[test]
fn automation_policy_and_parameter_keys_are_stable_and_bounded() {
    let graph = Pipeline {
        nodes: vec![
            PipelineNode::new(NodeId(1))
                .parameter("cutoff", AutomationPolicy::Continuous)
                .parameter("voice", AutomationPolicy::Structural),
        ],
        edges: vec![],
    };
    assert_eq!(
        graph.validate_automation(NodeId(1), "cutoff"),
        Ok(AutomationPolicy::Continuous)
    );
    assert!(matches!(
        graph.validate_automation(NodeId(1), "voice"),
        Err(PipelineError::UnsupportedAutomation {
            policy: AutomationPolicy::Structural,
            ..
        })
    ));
    assert!(matches!(
        graph.validate_automation(NodeId(1), "missing"),
        Err(PipelineError::UnknownParameter { .. })
    ));
}

#[test]
fn parameter_input_binds_one_driver_and_rejects_read_only_or_structural_targets() {
    let source = PipelineNode::new(NodeId(1)).output(Port::new(
        "value",
        PortType::Control(ControlDomain::Normalized { min: 0.0, max: 1.0 }),
    ));
    let target = PipelineNode::new(NodeId(2)).parameter_input(
        "cutoff",
        ControlDomain::Normalized { min: 0.0, max: 1.0 },
        AutomationPolicy::Continuous,
        false,
    );
    let graph = Pipeline {
        nodes: vec![source, target],
        edges: vec![edge(51, 1, "value", 2, "cutoff", EdgeRole::Control)],
    };
    assert!(graph.validate(limits()).is_ok());

    let readonly = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(3)).parameter_input(
            "meter",
            ControlDomain::Normalized { min: 0.0, max: 1.0 },
            AutomationPolicy::Continuous,
            true,
        )],
        edges: vec![],
    };
    assert!(matches!(
        readonly.validate_automation(NodeId(3), "meter"),
        Err(PipelineError::ReadOnlyParameter { .. })
    ));
}

#[test]
fn duplicate_parameter_keys_and_driver_port_aliases_are_rejected() {
    let duplicate = Pipeline {
        nodes: vec![
            PipelineNode::new(NodeId(1))
                .parameter("cutoff", AutomationPolicy::Continuous)
                .parameter("cutoff", AutomationPolicy::Continuous),
        ],
        edges: vec![],
    };
    assert!(matches!(
        duplicate.validate(limits()),
        Err(PipelineError::DuplicateParameter { .. })
    ));

    let alias = PipelineNode::new(NodeId(2))
        .parameter_input(
            "cutoff",
            ControlDomain::Normalized { min: 0.0, max: 1.0 },
            AutomationPolicy::Continuous,
            false,
        )
        .parameter_input(
            "resonance",
            ControlDomain::Normalized { min: 0.0, max: 1.0 },
            AutomationPolicy::Continuous,
            false,
        );
    let mut alias = alias;
    alias.parameters[1].port = alias.parameters[0].port.clone();
    let graph = Pipeline {
        nodes: vec![alias],
        edges: vec![],
    };
    assert!(matches!(
        graph.validate(limits()),
        Err(PipelineError::InvalidParameter { .. })
    ));
}

#[test]
fn role_and_direction_mismatches_are_reported_at_the_correct_endpoint() {
    let source = PipelineNode::new(NodeId(1))
        .input(Port::new("in", PortType::Events))
        .output(Port::new("out", PortType::Events));
    let target = PipelineNode::new(NodeId(2))
        .input(Port::new("in", PortType::Events))
        .output(Port::new("out", PortType::Events));
    let wrong_source_direction = Pipeline {
        nodes: vec![source.clone(), target.clone()],
        edges: vec![edge(61, 1, "in", 2, "in", EdgeRole::Events)],
    };
    assert!(matches!(
        wrong_source_direction.validate(limits()),
        Err(PipelineError::WrongDirection {
            node: NodeId(1),
            ..
        })
    ));

    let wrong_target_direction = Pipeline {
        nodes: vec![source, target],
        edges: vec![edge(62, 1, "out", 2, "out", EdgeRole::Events)],
    };
    assert!(matches!(
        wrong_target_direction.validate(limits()),
        Err(PipelineError::WrongDirection {
            node: NodeId(2),
            ..
        })
    ));

    let role_mismatch = Pipeline {
        nodes: vec![
            PipelineNode::new(NodeId(3)).output(Port::new("out", PortType::Events)),
            PipelineNode::new(NodeId(4)).input(Port::new("in", PortType::Events)),
        ],
        edges: vec![edge(63, 3, "out", 4, "in", EdgeRole::Control)],
    };
    assert!(matches!(
        role_mismatch.validate(limits()),
        Err(PipelineError::RoleMismatch { edge: EdgeId(63) })
    ));
}

#[test]
fn non_summing_event_and_control_inputs_reject_multiple_drivers() {
    let event_source =
        |id| PipelineNode::new(NodeId(id)).output(Port::new("out", PortType::Events));
    let event_graph = Pipeline {
        nodes: vec![
            event_source(1),
            event_source(2),
            PipelineNode::new(NodeId(3)).input(Port::new("in", PortType::Events)),
        ],
        edges: vec![
            edge(71, 1, "out", 3, "in", EdgeRole::Events),
            edge(72, 2, "out", 3, "in", EdgeRole::Events),
        ],
    };
    assert!(matches!(
        event_graph.validate(limits()),
        Err(PipelineError::ConflictingDrivers { .. })
    ));

    let control = ControlDomain::Normalized { min: 0.0, max: 1.0 };
    let control_graph = Pipeline {
        nodes: vec![
            PipelineNode::new(NodeId(4)).output(Port::new("out", PortType::Control(control))),
            PipelineNode::new(NodeId(5)).output(Port::new("out", PortType::Control(control))),
            PipelineNode::new(NodeId(6)).input(Port::new("in", PortType::Control(control))),
        ],
        edges: vec![
            edge(73, 4, "out", 6, "in", EdgeRole::Control),
            edge(74, 5, "out", 6, "in", EdgeRole::Control),
        ],
    };
    assert!(matches!(
        control_graph.validate(limits()),
        Err(PipelineError::ConflictingDrivers { .. })
    ));
}

#[test]
fn text_budget_counts_keys_and_parameter_port_references() {
    let graph = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(1)).parameter_input(
            "a-long-parameter-key",
            ControlDomain::Normalized { min: 0.0, max: 1.0 },
            AutomationPolicy::Continuous,
            false,
        )],
        edges: vec![],
    };
    assert!(matches!(
        graph.validate(PipelineLimits {
            text_bytes: 1,
            ..limits()
        }),
        Err(PipelineError::LimitExceeded {
            resource: "text_bytes",
            ..
        })
    ));
}

#[test]
fn empty_and_disconnected_graphs_are_valid_and_preserve_isolated_nodes() {
    let empty = Pipeline::default();
    assert!(
        empty
            .validate(PipelineLimits {
                nodes: 0,
                edges: 0,
                ports: 0,
                parameter_keys: 0,
                text_bytes: 0
            })
            .is_ok()
    );
    let isolated = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(9))],
        edges: vec![],
    };
    assert_eq!(
        isolated.validate(limits()).unwrap().topological_order,
        vec![NodeId(9)]
    );
}

#[test]
fn zero_is_a_valid_node_identity_and_port_keys_are_unique_across_directions() {
    let zero = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(0))],
        edges: vec![],
    };
    assert!(zero.validate(limits()).is_ok());

    let duplicate_port = Pipeline {
        nodes: vec![
            PipelineNode::new(NodeId(1))
                .input(Port::new("same", PortType::Events))
                .output(Port::new("same", PortType::Events)),
        ],
        edges: vec![],
    };
    assert!(matches!(
        duplicate_port.validate(limits()),
        Err(PipelineError::DuplicatePort { .. })
    ));
}

#[test]
fn port_roles_and_fan_in_contracts_are_rejected_even_without_edges() {
    let invalid_output = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(1)).output(Port {
            id: PortId::new("out"),
            ty: PortType::Audio(AudioLayout::Stereo),
            role: EdgeRole::Sidechain,
            accepts_multiple: false,
        })],
        edges: vec![],
    };
    assert!(matches!(
        invalid_output.validate(limits()),
        Err(PipelineError::InvalidPort { .. })
    ));

    let invalid_events = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(2)).input(Port {
            id: PortId::new("events"),
            ty: PortType::Events,
            role: EdgeRole::Main,
            accepts_multiple: false,
        })],
        edges: vec![],
    };
    assert!(matches!(
        invalid_events.validate(limits()),
        Err(PipelineError::InvalidPort { .. })
    ));

    let invalid_sum_output = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(3)).output(Port {
            id: PortId::new("audio"),
            ty: PortType::Audio(AudioLayout::Stereo),
            role: EdgeRole::Main,
            accepts_multiple: true,
        })],
        edges: vec![],
    };
    assert!(matches!(
        invalid_sum_output.validate(limits()),
        Err(PipelineError::InvalidPort { .. })
    ));
}

#[test]
fn one_automation_source_can_drive_two_independent_parameter_targets() {
    let source = PipelineNode::new(NodeId(1)).output(Port::new(
        "curve",
        PortType::Control(ControlDomain::Normalized { min: 0.0, max: 1.0 }),
    ));
    let target_a = PipelineNode::new(NodeId(2)).parameter_input(
        "cutoff",
        ControlDomain::Normalized { min: 0.0, max: 1.0 },
        AutomationPolicy::Continuous,
        false,
    );
    let target_b = PipelineNode::new(NodeId(3)).parameter_input(
        "cutoff",
        ControlDomain::Normalized { min: 0.0, max: 1.0 },
        AutomationPolicy::Continuous,
        false,
    );
    let graph = Pipeline {
        nodes: vec![target_b, source, target_a],
        edges: vec![
            edge(81, 1, "curve", 2, "cutoff", EdgeRole::Control),
            edge(82, 1, "curve", 3, "cutoff", EdgeRole::Control),
        ],
    };
    assert!(graph.validate(limits()).is_ok());
}

#[test]
fn two_parameter_targets_cannot_alias_one_driver_port() {
    let node = PipelineNode {
        id: NodeId(4),
        inputs: vec![Port::new(
            "cutoff",
            PortType::Control(ControlDomain::Normalized { min: 0.0, max: 1.0 }),
        )],
        outputs: vec![],
        parameters: vec![
            Parameter {
                key: "a".into(),
                port: Some(PortId::new("cutoff")),
                policy: AutomationPolicy::Continuous,
                read_only: false,
            },
            Parameter {
                key: "b".into(),
                port: Some(PortId::new("cutoff")),
                policy: AutomationPolicy::Continuous,
                read_only: false,
            },
        ],
    };
    assert!(matches!(
        (Pipeline {
            nodes: vec![node],
            edges: vec![]
        })
        .validate(limits()),
        Err(PipelineError::InvalidParameter { .. })
    ));
}

#[test]
fn resource_budgets_accept_exact_counts_and_reject_one_below() {
    let graph = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(1)).output(Port::new("x", PortType::Events))],
        edges: vec![],
    };
    let exact = PipelineLimits {
        nodes: 1,
        edges: 0,
        ports: 1,
        parameter_keys: 0,
        text_bytes: 1,
    };
    assert!(graph.validate(exact).is_ok());
    assert!(matches!(
        graph.validate(PipelineLimits { nodes: 0, ..exact }),
        Err(PipelineError::LimitExceeded {
            resource: "nodes",
            ..
        })
    ));
    assert!(matches!(
        graph.validate(PipelineLimits { ports: 0, ..exact }),
        Err(PipelineError::LimitExceeded {
            resource: "ports",
            ..
        })
    ));
}

#[test]
fn conflicting_driver_error_preserves_both_parallel_edge_ids() {
    let control = ControlDomain::Normalized { min: 0.0, max: 1.0 };
    let graph = Pipeline {
        nodes: vec![
            PipelineNode::new(NodeId(1)).output(Port::new("out", PortType::Control(control))),
            PipelineNode::new(NodeId(2)).input(Port::new("in", PortType::Control(control))),
        ],
        edges: vec![
            edge(91, 1, "out", 2, "in", EdgeRole::Control),
            edge(92, 1, "out", 2, "in", EdgeRole::Control),
        ],
    };
    assert_eq!(
        graph.validate(limits()),
        Err(PipelineError::ConflictingDrivers {
            node: NodeId(2),
            port: PortId::new("in"),
            first: EdgeId(91),
            second: EdgeId(92),
        })
    );
}

#[test]
fn duplicate_and_dangling_handles_are_rejected_before_graph_walk() {
    let duplicate = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(1)), PipelineNode::new(NodeId(1))],
        edges: vec![],
    };
    assert_eq!(
        duplicate.validate(limits()),
        Err(PipelineError::DuplicateNode(NodeId(1)))
    );

    let dangling = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(1)).output(Port::new("out", PortType::Events))],
        edges: vec![edge(41, 1, "out", 99, "in", EdgeRole::Events)],
    };
    assert_eq!(
        dangling.validate(limits()),
        Err(PipelineError::MissingNode {
            edge: EdgeId(41),
            node: NodeId(99),
        })
    );
}

#[test]
fn resource_limits_are_checked_before_port_validation() {
    let graph = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(1)).output(Port::new("out", PortType::Events))],
        edges: vec![],
    };
    assert_eq!(
        graph.validate(PipelineLimits {
            nodes: 0,
            ..limits()
        }),
        Err(PipelineError::LimitExceeded {
            resource: "nodes",
            found: 1,
            limit: 0,
        })
    );
}

#[test]
fn long_cycle_reports_a_connected_edge_path_without_recursion() {
    let count = 10_000_u64;
    let nodes = (0..count)
        .map(|id| {
            PipelineNode::new(NodeId(id))
                .input(Port::new("in", PortType::Events))
                .output(Port::new("out", PortType::Events))
        })
        .collect();
    let mut edges = (0..count)
        .map(|id| edge(id + 1, id, "out", (id + 1) % count, "in", EdgeRole::Events))
        .collect::<Vec<_>>();
    edges.reverse();
    let graph = Pipeline { nodes, edges };

    let Err(PipelineError::Cycle { nodes, edges }) = graph.validate(PipelineLimits {
        nodes: count as usize,
        edges: count as usize,
        ports: count as usize * 2,
        parameter_keys: 0,
        text_bytes: count as usize * 10,
    }) else {
        panic!("long cycle must be rejected");
    };
    assert_eq!(nodes.first(), nodes.last());
    assert_eq!(nodes.len(), edges.len() + 1);
    for window in nodes.windows(2).zip(edges.iter()) {
        let (pair, edge) = window;
        assert_eq!(edge.0, pair[0].0 + 1);
        assert_eq!(pair[1].0, (pair[0].0 + 1) % count);
    }
}

#[test]
fn cycle_report_excludes_a_smaller_downstream_dead_end() {
    let graph = Pipeline {
        nodes: vec![
            PipelineNode::new(NodeId(1)).input(Port::new("in", PortType::Events)),
            PipelineNode::new(NodeId(2))
                .input(Port::new("in", PortType::Events))
                .output(Port::new("out", PortType::Events)),
            PipelineNode::new(NodeId(3))
                .input(Port::new("in", PortType::Events))
                .output(Port::new("out", PortType::Events)),
        ],
        edges: vec![
            edge(1, 2, "out", 1, "in", EdgeRole::Events),
            edge(2, 2, "out", 3, "in", EdgeRole::Events),
            edge(3, 3, "out", 2, "in", EdgeRole::Events),
        ],
    };
    let Err(PipelineError::Cycle { nodes, edges }) = graph.validate(limits()) else {
        panic!("a cycle must be rejected");
    };
    assert!(
        !edges.is_empty(),
        "the report must describe a real cycle: {nodes:?}"
    );
    assert_eq!(nodes.first(), nodes.last());
    assert!(
        !nodes.contains(&NodeId(1)),
        "a dead end is not part of a cycle"
    );
}
