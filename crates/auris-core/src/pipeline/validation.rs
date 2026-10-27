use std::collections::{BTreeMap, BTreeSet};

use super::{
    AudioLayout, ControlDomain, EdgeId, EdgeRole, NodeId, Parameter, Pipeline, PipelineEdge,
    PipelineError, PipelineLimits, Port, PortId, PortType, Validation,
};

#[derive(Copy, Clone, PartialEq, Eq)]
enum Direction {
    Input,
    Output,
}

#[derive(Default)]
struct Interfaces<'a> {
    nodes: BTreeMap<NodeId, usize>,
    ports: BTreeMap<(NodeId, &'a PortId), (Direction, &'a Port)>,
    parameters: BTreeMap<(NodeId, &'a PortId), &'a Parameter>,
}

pub(super) fn validate(
    graph: &Pipeline,
    limits: PipelineLimits,
) -> Result<Validation, PipelineError> {
    preflight(graph, limits)?;
    let interfaces = index_interfaces(graph)?;
    let mut edge_ids = BTreeSet::new();
    let mut drivers = BTreeMap::new();
    let mut adjacency = vec![Vec::new(); graph.nodes.len()];
    for edge in &graph.edges {
        if !edge_ids.insert(edge.id) {
            return Err(PipelineError::DuplicateEdge(edge.id));
        }
        let from = node_index(&interfaces, edge.id, edge.from.0)?;
        let to = node_index(&interfaces, edge.id, edge.to.0)?;
        let output = endpoint(&interfaces, edge, Direction::Output)?;
        let input = endpoint(&interfaces, edge, Direction::Input)?;
        if output.ty != input.ty {
            return Err(PipelineError::TypeMismatch { edge: edge.id });
        }
        if edge.role != input.role {
            return Err(PipelineError::RoleMismatch { edge: edge.id });
        }
        if !input.accepts_multiple
            && let Some(first) = drivers.insert((edge.to.0, &edge.to.1), edge.id)
        {
            return Err(PipelineError::ConflictingDrivers {
                node: edge.to.0,
                port: edge.to.1.clone(),
                first,
                second: edge.id,
            });
        }
        if let Some(parameter) = interfaces.parameters.get(&(edge.to.0, &edge.to.1)) {
            parameter.automation_policy(edge.to.0)?;
        }
        adjacency[from].push((to, edge.id));
    }
    for targets in &mut adjacency {
        targets.sort_unstable_by_key(|&(index, edge)| (graph.nodes[index].id, edge));
    }
    dependency_order(graph, &interfaces, &adjacency)
}

fn check_limit(resource: &'static str, found: usize, limit: usize) -> Result<(), PipelineError> {
    if found > limit {
        Err(PipelineError::LimitExceeded {
            resource,
            found,
            limit,
        })
    } else {
        Ok(())
    }
}

fn preflight(graph: &Pipeline, limits: PipelineLimits) -> Result<(), PipelineError> {
    check_limit("nodes", graph.nodes.len(), limits.nodes)?;
    check_limit("edges", graph.edges.len(), limits.edges)?;
    let ports = graph.nodes.iter().fold(0usize, |count, node| {
        count
            .saturating_add(node.inputs.len())
            .saturating_add(node.outputs.len())
    });
    let parameters = graph.nodes.iter().fold(0usize, |count, node| {
        count.saturating_add(node.parameters.len())
    });
    check_limit("ports", ports, limits.ports)?;
    check_limit("parameter_keys", parameters, limits.parameter_keys)?;
    let mut text_bytes = 0usize;
    for node in &graph.nodes {
        for port in node.inputs.iter().chain(&node.outputs) {
            text_bytes = text_bytes.saturating_add(port.id.0.len());
        }
        for parameter in &node.parameters {
            text_bytes = text_bytes
                .saturating_add(parameter.key.len())
                .saturating_add(parameter.port.as_ref().map_or(0, |port| port.0.len()));
        }
    }
    for edge in &graph.edges {
        text_bytes = text_bytes
            .saturating_add(edge.from.1.0.len())
            .saturating_add(edge.to.1.0.len());
    }
    check_limit("text_bytes", text_bytes, limits.text_bytes)
}

fn index_interfaces(graph: &Pipeline) -> Result<Interfaces<'_>, PipelineError> {
    let mut interfaces = Interfaces::default();
    for (index, node) in graph.nodes.iter().enumerate() {
        if interfaces.nodes.insert(node.id, index).is_some() {
            return Err(PipelineError::DuplicateNode(node.id));
        }
        for (direction, ports) in [
            (Direction::Input, &node.inputs),
            (Direction::Output, &node.outputs),
        ] {
            for port in ports {
                validate_port(node.id, port, direction)?;
                if interfaces
                    .ports
                    .insert((node.id, &port.id), (direction, port))
                    .is_some()
                {
                    return Err(PipelineError::DuplicatePort {
                        node: node.id,
                        port: port.id.clone(),
                    });
                }
            }
        }
        let mut keys = BTreeSet::new();
        for parameter in &node.parameters {
            if parameter.key.is_empty() {
                return Err(invalid_parameter(
                    node.id,
                    parameter,
                    "key must not be empty",
                ));
            }
            if !keys.insert(parameter.key.as_str()) {
                return Err(PipelineError::DuplicateParameter {
                    node: node.id,
                    key: parameter.key.clone(),
                });
            }
            if let Some(port) = &parameter.port {
                if !matches!(
                    interfaces.ports.get(&(node.id, port)),
                    Some((
                        Direction::Input,
                        Port {
                            ty: PortType::Control(_),
                            ..
                        }
                    ))
                ) {
                    return Err(invalid_parameter(
                        node.id,
                        parameter,
                        "driver port must name a control input",
                    ));
                }
                if interfaces
                    .parameters
                    .insert((node.id, port), parameter)
                    .is_some()
                {
                    return Err(invalid_parameter(
                        node.id,
                        parameter,
                        "driver port already belongs to another parameter",
                    ));
                }
            }
        }
    }
    Ok(interfaces)
}

fn invalid_parameter(node: NodeId, parameter: &Parameter, reason: &'static str) -> PipelineError {
    PipelineError::InvalidParameter {
        node,
        key: parameter.key.clone(),
        reason,
    }
}

fn validate_port(node: NodeId, port: &Port, direction: Direction) -> Result<(), PipelineError> {
    let invalid = |reason| PipelineError::InvalidPort {
        node,
        port: port.id.clone(),
        reason,
    };
    if port.id.0.is_empty() {
        return Err(invalid("key must not be empty"));
    }
    let valid_role = match port.ty {
        PortType::Events => port.role == EdgeRole::Events,
        PortType::Control(_) => port.role == EdgeRole::Control,
        PortType::Audio(_) => {
            port.role == EdgeRole::Main
                || (direction == Direction::Input && port.role == EdgeRole::Sidechain)
        }
    };
    if !valid_role {
        return Err(invalid("role must match the port type and direction"));
    }
    if port.accepts_multiple
        && (direction != Direction::Input || !matches!(port.ty, PortType::Audio(_)))
    {
        return Err(invalid("only audio inputs may declare summing"));
    }
    let valid_range = match port.ty {
        PortType::Audio(AudioLayout::Discrete(channels)) => channels >= 3,
        PortType::Control(domain) => {
            let (min, max) = match domain {
                ControlDomain::Native { min, max, .. } | ControlDomain::Normalized { min, max } => {
                    (min, max)
                }
            };
            min.is_finite()
                && max.is_finite()
                && min < max
                && (!matches!(domain, ControlDomain::Normalized { .. })
                    || (min >= 0.0 && max <= 1.0))
        }
        _ => true,
    };
    if !valid_range {
        return Err(PipelineError::InvalidPortRange {
            node,
            port: port.id.clone(),
        });
    }
    Ok(())
}

fn node_index(
    interfaces: &Interfaces<'_>,
    edge: EdgeId,
    node: NodeId,
) -> Result<usize, PipelineError> {
    interfaces
        .nodes
        .get(&node)
        .copied()
        .ok_or(PipelineError::MissingNode { edge, node })
}

fn endpoint<'a>(
    interfaces: &Interfaces<'a>,
    edge: &PipelineEdge,
    direction: Direction,
) -> Result<&'a Port, PipelineError> {
    let (node, port) = match direction {
        Direction::Input => &edge.to,
        Direction::Output => &edge.from,
    };
    let (actual, descriptor) =
        interfaces
            .ports
            .get(&(*node, port))
            .ok_or_else(|| PipelineError::MissingPort {
                edge: edge.id,
                node: *node,
                port: port.clone(),
            })?;
    if *actual != direction {
        return Err(PipelineError::WrongDirection {
            edge: edge.id,
            node: *node,
            port: port.clone(),
        });
    }
    Ok(descriptor)
}

fn dependency_order(
    graph: &Pipeline,
    interfaces: &Interfaces<'_>,
    adjacency: &[Vec<(usize, EdgeId)>],
) -> Result<Validation, PipelineError> {
    let mut indegree = vec![0usize; graph.nodes.len()];
    for targets in adjacency {
        for &(to, _) in targets {
            indegree[to] += 1;
        }
    }
    let mut ready: BTreeSet<_> = interfaces
        .nodes
        .iter()
        .filter_map(|(&id, &index)| (indegree[index] == 0).then_some((id, index)))
        .collect();
    let mut order = Vec::with_capacity(graph.nodes.len());
    while let Some((id, index)) = ready.pop_first() {
        order.push(id);
        for &(to, _) in &adjacency[index] {
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.insert((graph.nodes[to].id, to));
            }
        }
    }
    if order.len() != graph.nodes.len() {
        return Err(find_cycle(graph, interfaces, adjacency)
            .unwrap_or(PipelineError::InconsistentDependencies));
    }
    Ok(Validation {
        topological_order: order,
    })
}

fn find_cycle(
    graph: &Pipeline,
    interfaces: &Interfaces<'_>,
    adjacency: &[Vec<(usize, EdgeId)>],
) -> Option<PipelineError> {
    // Active positions identify the start of a real cycle, excluding downstream residuals
    // left by Kahn's algorithm. Explicit stack frames keep deep graphs off the call stack.
    let mut visited = vec![false; graph.nodes.len()];
    let mut active = vec![None; graph.nodes.len()];
    let mut stack = Vec::<(usize, usize)>::new();
    let mut path_edges = Vec::new();
    for &start in interfaces.nodes.values() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        active[start] = Some(0);
        stack.push((start, 0));
        while let Some((node, offset)) = stack.last_mut() {
            let Some(&(next, edge)) = adjacency[*node].get(*offset) else {
                active[*node] = None;
                stack.pop();
                path_edges.pop();
                continue;
            };
            *offset += 1;
            if let Some(position) = active[next] {
                let mut nodes: Vec<_> = stack[position..]
                    .iter()
                    .map(|&(index, _)| graph.nodes[index].id)
                    .collect();
                nodes.push(graph.nodes[next].id);
                let mut edges = path_edges[position..].to_vec();
                edges.push(edge);
                return Some(PipelineError::Cycle { nodes, edges });
            }
            if !visited[next] {
                visited[next] = true;
                active[next] = Some(stack.len());
                stack.push((next, 0));
                path_edges.push(edge);
            }
        }
    }
    None
}
