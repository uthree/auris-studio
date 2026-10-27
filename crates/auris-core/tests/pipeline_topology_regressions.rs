use auris_core::ParamUnit;
use auris_core::pipeline::*;

fn limits() -> PipelineLimits {
    PipelineLimits {
        nodes: 64,
        edges: 256,
        ports: 128,
        parameter_keys: 32,
        text_bytes: 8_192,
    }
}

fn edge(id: u64, from: u64, to: u64, role: EdgeRole) -> PipelineEdge {
    PipelineEdge {
        id: EdgeId(id),
        from: (NodeId(from), PortId::new("out")),
        to: (NodeId(to), PortId::new("in")),
        role,
    }
}

fn audio_node(id: u64) -> PipelineNode {
    PipelineNode::new(NodeId(id))
        .input(Port::sum_input("in", AudioLayout::Stereo))
        .output(Port::new("out", PortType::Audio(AudioLayout::Stereo)))
}

#[test]
fn all_four_node_digraphs_match_independent_reachability_and_closed_cycle_checks() {
    // The transitive-closure oracle is independent of the validator's traversal/order logic.
    // Include self loops, fan-in, fan-out, parallel-ready nodes and disconnected components.
    for mask in 0u32..(1 << 16) {
        let mut reach = [[false; 4]; 4];
        let mut graph = Pipeline {
            nodes: (0..4).rev().map(audio_node).collect(),
            edges: Vec::new(),
        };
        for (from, row) in reach.iter_mut().enumerate() {
            for (to, connected) in row.iter_mut().enumerate() {
                if mask & (1 << (from * 4 + to)) != 0 {
                    *connected = true;
                    graph.edges.push(edge(
                        (from * 4 + to) as u64,
                        from as u64,
                        to as u64,
                        EdgeRole::Main,
                    ));
                }
            }
        }
        graph.edges.reverse();
        for through in 0..4 {
            for from in 0..4 {
                for to in 0..4 {
                    reach[from][to] |= reach[from][through] && reach[through][to];
                }
            }
        }
        let cyclic = (0..4).any(|node| reach[node][node]);
        match graph.validate(limits()) {
            Ok(valid) => {
                assert!(!cyclic, "accepted cycle in mask {mask}");
                assert_eq!(valid.topological_order.len(), 4);
                let mut seen = std::collections::BTreeSet::new();
                for node in &valid.topological_order {
                    assert!(seen.insert(*node));
                    assert!(
                        graph
                            .edges
                            .iter()
                            .filter(|edge| edge.to.0 == *node)
                            .all(|edge| seen.contains(&edge.from.0))
                    );
                }
            }
            Err(PipelineError::Cycle { nodes, edges }) => {
                assert!(cyclic, "false cycle in mask {mask}");
                assert!(!edges.is_empty());
                assert_eq!(nodes.len(), edges.len() + 1);
                assert_eq!(nodes.first(), nodes.last());
                for (pair, id) in nodes.windows(2).zip(edges) {
                    assert!(graph.edges.iter().any(|edge| edge.id == id
                        && edge.from.0 == pair[0]
                        && edge.to.0 == pair[1]));
                }
            }
            other => panic!("unexpected result for mask {mask}: {other:?}"),
        }
    }
}

#[test]
fn event_audio_control_cycle_is_rejected_as_one_dependency_graph() {
    let control = ControlDomain::Native {
        unit: ParamUnit::Decibels,
        min: -60.,
        max: 12.,
    };
    let graph = Pipeline {
        nodes: vec![
            PipelineNode::new(NodeId(10))
                .input(Port::new("in", PortType::Control(control)))
                .output(Port::new("out", PortType::Events)),
            PipelineNode::new(NodeId(20))
                .input(Port::new("in", PortType::Events))
                .output(Port::new("out", PortType::Audio(AudioLayout::Stereo))),
            PipelineNode::new(NodeId(30))
                .input(Port::new("in", PortType::Audio(AudioLayout::Stereo)))
                .output(Port::new("out", PortType::Control(control))),
        ],
        edges: vec![
            edge(1, 10, 20, EdgeRole::Events),
            edge(2, 20, 30, EdgeRole::Main),
            edge(3, 30, 10, EdgeRole::Control),
        ],
    };
    assert_eq!(
        graph.validate(limits()),
        Err(PipelineError::Cycle {
            nodes: vec![NodeId(10), NodeId(20), NodeId(30), NodeId(10)],
            edges: vec![EdgeId(1), EdgeId(2), EdgeId(3)],
        })
    );
}

#[test]
fn ready_node_ties_and_candidate_reordering_do_not_change_dependency_order() {
    let mut graph = Pipeline {
        nodes: vec![audio_node(8), audio_node(5), audio_node(1)],
        edges: vec![edge(1, 1, 5, EdgeRole::Main)],
    };
    let expected = vec![NodeId(1), NodeId(5), NodeId(8)];
    assert_eq!(
        graph.validate(limits()).unwrap().topological_order,
        expected
    );
    graph.nodes.reverse();
    graph.edges.reverse();
    assert_eq!(
        graph.validate(limits()).unwrap().topological_order,
        expected
    );
}

#[test]
fn incoming_edges_cannot_bypass_read_only_or_static_parameter_policies() {
    let domain = ControlDomain::Normalized { min: 0., max: 1. };
    for (policy, read_only) in [
        (AutomationPolicy::Continuous, true),
        (AutomationPolicy::PreparationOnly, false),
        (AutomationPolicy::RegenerationRequired, false),
        (AutomationPolicy::Structural, false),
    ] {
        let graph = Pipeline {
            nodes: vec![
                PipelineNode::new(NodeId(1)).output(Port::new("out", PortType::Control(domain))),
                PipelineNode::new(NodeId(2)).parameter_input("in", domain, policy, read_only),
            ],
            edges: vec![edge(70, 1, 2, EdgeRole::Control)],
        };
        let expected = if read_only {
            PipelineError::ReadOnlyParameter {
                node: NodeId(2),
                key: "in".into(),
            }
        } else {
            PipelineError::UnsupportedAutomation {
                node: NodeId(2),
                key: "in".into(),
                policy,
            }
        };
        assert_eq!(graph.validate(limits()), Err(expected));
    }
}

#[test]
fn layout_units_and_native_ranges_need_explicit_conversion() {
    let stereo = PortType::Audio(AudioLayout::Stereo);
    let hz = |min| {
        PortType::Control(ControlDomain::Native {
            unit: ParamUnit::Hertz,
            min,
            max: 20_000.,
        })
    };
    for (from, to, role) in [
        (stereo, PortType::Audio(AudioLayout::Mono), EdgeRole::Main),
        (hz(20.), hz(40.), EdgeRole::Control),
        (
            hz(20.),
            PortType::Control(ControlDomain::Native {
                unit: ParamUnit::Seconds,
                min: 20.,
                max: 20_000.,
            }),
            EdgeRole::Control,
        ),
    ] {
        let graph = Pipeline {
            nodes: vec![
                PipelineNode::new(NodeId(1)).output(Port::new("out", from)),
                PipelineNode::new(NodeId(2)).input(Port::new("in", to)),
            ],
            edges: vec![edge(1, 1, 2, role)],
        };
        assert_eq!(
            graph.validate(limits()),
            Err(PipelineError::TypeMismatch { edge: EdgeId(1) })
        );
    }
}

#[test]
fn malformed_ranges_and_noncanonical_discrete_layouts_are_rejected() {
    for ty in [
        PortType::Audio(AudioLayout::Discrete(0)),
        PortType::Audio(AudioLayout::Discrete(2)),
        PortType::Control(ControlDomain::Normalized { min: -0.1, max: 1. }),
        PortType::Control(ControlDomain::Normalized { min: 0., max: 1.1 }),
        PortType::Control(ControlDomain::Normalized { min: 1., max: 0. }),
        PortType::Control(ControlDomain::Normalized {
            min: 0.,
            max: f32::INFINITY,
        }),
        PortType::Control(ControlDomain::Native {
            unit: ParamUnit::Plain,
            min: f32::NAN,
            max: 1.,
        }),
    ] {
        let graph = Pipeline {
            nodes: vec![PipelineNode::new(NodeId(1)).output(Port::new("out", ty))],
            edges: vec![],
        };
        assert_eq!(
            graph.validate(limits()),
            Err(PipelineError::InvalidPortRange {
                node: NodeId(1),
                port: PortId::new("out")
            })
        );
    }
}

#[test]
fn missing_ports_are_distinct_from_wrong_direction_on_either_endpoint() {
    let mut graph = Pipeline {
        nodes: vec![audio_node(1), audio_node(2)],
        edges: vec![edge(6, 1, 2, EdgeRole::Main)],
    };
    graph.edges[0].from.1 = PortId::new("missing");
    assert_eq!(
        graph.validate(limits()),
        Err(PipelineError::MissingPort {
            edge: EdgeId(6),
            node: NodeId(1),
            port: PortId::new("missing")
        })
    );
    graph.edges[0].from.1 = PortId::new("out");
    graph.edges[0].to.1 = PortId::new("missing");
    assert_eq!(
        graph.validate(limits()),
        Err(PipelineError::MissingPort {
            edge: EdgeId(6),
            node: NodeId(2),
            port: PortId::new("missing")
        })
    );
}

#[test]
fn duplicate_edge_ids_do_not_become_parallel_routes() {
    let graph = Pipeline {
        nodes: vec![audio_node(1), audio_node(2)],
        edges: vec![edge(7, 1, 2, EdgeRole::Main), edge(7, 1, 2, EdgeRole::Main)],
    };
    assert_eq!(
        graph.validate(limits()),
        Err(PipelineError::DuplicateEdge(EdgeId(7)))
    );
}

#[test]
fn edge_and_parameter_limits_precede_reference_validation() {
    let graph = Pipeline {
        nodes: vec![PipelineNode::new(NodeId(1)).parameter("x", AutomationPolicy::Continuous)],
        edges: vec![edge(1, 1, 99, EdgeRole::Events)],
    };
    assert_eq!(
        graph.validate(PipelineLimits {
            edges: 0,
            ..limits()
        }),
        Err(PipelineError::LimitExceeded {
            resource: "edges",
            found: 1,
            limit: 0
        })
    );
    assert_eq!(
        graph.validate(PipelineLimits {
            parameter_keys: 0,
            ..limits()
        }),
        Err(PipelineError::LimitExceeded {
            resource: "parameter_keys",
            found: 1,
            limit: 0
        })
    );
}

#[test]
fn summed_audio_parallel_routes_count_separately_in_dependency_order() {
    let graph = Pipeline {
        nodes: vec![audio_node(1), audio_node(2), audio_node(3)],
        edges: vec![
            edge(1, 1, 2, EdgeRole::Main),
            edge(2, 1, 2, EdgeRole::Main),
            edge(3, 2, 3, EdgeRole::Main),
        ],
    };
    assert_eq!(
        graph.validate(limits()).unwrap().topological_order,
        vec![NodeId(1), NodeId(2), NodeId(3)]
    );
}
