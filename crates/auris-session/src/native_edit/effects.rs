//! Identity-based edits for conventional insert chains and sidechain edges.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use auris_core::pipeline::content::{
    ContentLimits, GroupId, PluginInstance, Processor, ProcessorKind,
};
use auris_core::pipeline::document::NativeDocument;
use auris_core::pipeline::{EdgeId, EdgeRole, NodeId, PipelineEdge, PortId};
use auris_core::project::{EffectSlotId, TrackId};

use super::{NativeCreated, NativeEditError};

/// An edit to one conventional group's effect chain.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum NativeEffectEdit {
    /// Inserts a new effect at the zero-based position in the group's chain.
    Insert {
        /// Owning processing group.
        group: GroupId,
        /// Insert position, where zero is immediately after the group's sum node.
        index: usize,
        /// Persisted plugin payload; capability checks happen during preparation.
        plugin: PluginInstance,
    },
    /// Removes an existing effect processor and its owned routing edges.
    Remove {
        /// Effect processor identity.
        node: NodeId,
    },
    /// Moves an existing effect to another zero-based position in its group.
    Move {
        /// Effect processor identity.
        node: NodeId,
        /// New chain position.
        index: usize,
    },
    /// Adds, retargets or removes the sidechain source of an effect.
    SetSidechain {
        /// Effect processor identity.
        node: NodeId,
        /// Source track, or `None` to remove the sidechain.
        source: Option<TrackId>,
    },
}

/// Applies one effect edit to a cloned native candidate.
pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativeEffectEdit,
    limits: auris_core::pipeline::content::ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    validate_acyclic(document)?;
    let created = match edit {
        NativeEffectEdit::Insert {
            group,
            index,
            plugin,
        } => insert(document, *group, *index, plugin, limits)?,
        NativeEffectEdit::Remove { node } => remove(document, *node)?,
        NativeEffectEdit::Move { node, index } => {
            move_effect(document, *node, *index)?;
            Vec::new()
        }
        NativeEffectEdit::SetSidechain { node, source } => {
            set_sidechain(document, *node, *source, limits)?
        }
    };
    validate_acyclic(document)?;
    Ok(created)
}

fn insert(
    document: &mut NativeDocument,
    group: GroupId,
    index: usize,
    plugin: &PluginInstance,
    limits: auris_core::pipeline::content::ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    super::plugins::validate_plugin(plugin)?;
    let chain = chain(document, group)?;
    if index > chain.nodes.len() {
        return Err(NativeEditError::Invalid(
            "effect insert index is out of range".into(),
        ));
    }
    if document.content.processors.len() >= limits.processors {
        return Err(auris_core::pipeline::content::ContentError::Limit("processors").into());
    }
    if document.content.edges.len().saturating_add(1) > limits.edges {
        return Err(auris_core::pipeline::content::ContentError::Limit("edges").into());
    }
    let node = NodeId(take_id(document)?);
    let slot = EffectSlotId(take_id(document)?);
    let edge_b = take_id(document)?;
    let previous = if index == 0 {
        chain.input
    } else {
        chain.nodes[index - 1]
    };
    let next = if index == chain.nodes.len() {
        chain.strip
    } else {
        chain.nodes[index]
    };
    let old = edge_between_mut(document, previous, next)?;
    old.to = (node, PortId::new("audio_in"));
    document.content.edges.push(PipelineEdge {
        id: EdgeId(edge_b),
        from: (node, PortId::new("audio_out")),
        to: (next, PortId::new("audio_in")),
        role: EdgeRole::Main,
    });
    document.content.processors.push(Processor {
        id: node,
        group: Some(group),
        kind: ProcessorKind::Effect {
            slot,
            plugin: plugin.clone(),
            enabled: true,
        },
    });
    Ok(vec![
        NativeCreated::Node(node),
        NativeCreated::Effect(slot),
        NativeCreated::Edge(EdgeId(edge_b)),
    ])
}

fn remove(
    document: &mut NativeDocument,
    node: NodeId,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let processor = document
        .content
        .processors
        .iter()
        .find(|p| p.id == node)
        .ok_or_else(|| missing_node(node))?;
    if !matches!(&processor.kind, ProcessorKind::Effect { .. }) {
        return Err(NativeEditError::Invalid(
            "target processor is not an effect".into(),
        ));
    }
    let group = processor
        .group
        .ok_or_else(|| NativeEditError::Invalid("effect has no owning group".into()))?;
    let chain = chain(document, group)?;
    let position = chain
        .nodes
        .iter()
        .position(|candidate| *candidate == node)
        .ok_or_else(|| {
            NativeEditError::Invalid("effect is not in its conventional insert chain".into())
        })?;
    let previous = if position == 0 {
        chain.input
    } else {
        chain.nodes[position - 1]
    };
    let next = if position + 1 == chain.nodes.len() {
        chain.strip
    } else {
        chain.nodes[position + 1]
    };
    let incoming = edge_between(document, previous, node)?.id;
    let outgoing = edge_between(document, node, next)?.id;
    document.content.edges.retain(|edge| edge.id != outgoing);
    let retained = document
        .content
        .edges
        .iter_mut()
        .find(|edge| edge.id == incoming)
        .ok_or_else(|| NativeEditError::Invalid("effect input edge disappeared".into()))?;
    retained.to = (next, PortId::new("audio_in"));
    document
        .content
        .edges
        .retain(|edge| edge.from.0 != node && edge.to.0 != node);
    document
        .content
        .automation_bindings
        .retain(|binding| binding.target.node != node);
    document
        .content
        .processors
        .retain(|processor| processor.id != node);
    Ok(Vec::new())
}

fn move_effect(
    document: &mut NativeDocument,
    node: NodeId,
    index: usize,
) -> Result<(), NativeEditError> {
    let processor = document
        .content
        .processors
        .iter()
        .find(|p| p.id == node)
        .ok_or_else(|| missing_node(node))?;
    if !matches!(&processor.kind, ProcessorKind::Effect { .. }) {
        return Err(NativeEditError::Invalid(
            "target processor is not an effect".into(),
        ));
    }
    let group = processor
        .group
        .ok_or_else(|| NativeEditError::Invalid("effect has no owning group".into()))?;
    let chain = chain(document, group)?;
    let Some(old_index) = chain.nodes.iter().position(|candidate| *candidate == node) else {
        return Err(NativeEditError::Invalid(
            "effect is not in its conventional insert chain".into(),
        ));
    };
    if index >= chain.nodes.len() {
        return Err(NativeEditError::Invalid(
            "effect move index is out of range".into(),
        ));
    }
    if old_index == index {
        return Ok(());
    }
    let mut order = chain.nodes;
    let moved = order.remove(old_index);
    order.insert(index, moved);
    for (edge, pair) in chain.edges.iter().zip(
        std::iter::once(chain.input)
            .chain(order.iter().copied())
            .zip(order.iter().copied().chain(std::iter::once(chain.strip))),
    ) {
        let target = document
            .content
            .edges
            .iter_mut()
            .find(|candidate| candidate.id == edge.id)
            .ok_or_else(|| NativeEditError::Invalid("effect chain edge disappeared".into()))?;
        target.from.0 = pair.0;
        target.to.0 = pair.1;
    }
    Ok(())
}

fn set_sidechain(
    document: &mut NativeDocument,
    node: NodeId,
    source: Option<TrackId>,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let processor = document
        .content
        .processors
        .iter()
        .find(|p| p.id == node)
        .ok_or_else(|| missing_node(node))?;
    if !matches!(&processor.kind, ProcessorKind::Effect { .. }) {
        return Err(NativeEditError::Invalid(
            "target processor is not an effect".into(),
        ));
    }
    let group = processor
        .group
        .ok_or_else(|| NativeEditError::Invalid("effect has no owning group".into()))?;
    let chain = chain(document, group)?;
    if !chain.nodes.contains(&node) {
        return Err(NativeEditError::Invalid(
            "effect is not in its conventional insert chain".into(),
        ));
    }
    let source_node = source
        .map(|track| strip_for_track(document, track))
        .transpose()?;
    let sidechains: Vec<_> = document
        .content
        .edges
        .iter()
        .enumerate()
        .filter(|(_, edge)| {
            edge.to.0 == node
                && edge.to.1 == PortId::new("sidechain_in")
                && edge.role == EdgeRole::Sidechain
        })
        .map(|(index, _)| index)
        .collect();
    if sidechains.len() > 1 {
        return Err(NativeEditError::Invalid(
            "effect has multiple sidechain edges".into(),
        ));
    }
    if document.content.edges.iter().any(|edge| {
        edge.to.0 == node
            && edge.role == EdgeRole::Sidechain
            && edge.to.1 != PortId::new("sidechain_in")
    }) {
        return Err(NativeEditError::Invalid(
            "effect has an invalid sidechain edge".into(),
        ));
    }
    let existing = sidechains.into_iter().next();
    let creates_edge = source_node.is_some() && existing.is_none();
    let mut created_id = None;
    match (source_node, existing) {
        (None, Some(position)) => {
            document.content.edges.remove(position);
        }
        (Some(source_node), Some(position)) => {
            document.content.edges[position].from = (source_node, PortId::new("audio_out"));
        }
        (Some(source_node), None) => {
            if document.content.edges.len() >= limits.edges {
                return Err(auris_core::pipeline::content::ContentError::Limit("edges").into());
            }
            let id = take_id(document)?;
            created_id = Some(id);
            document.content.edges.push(PipelineEdge {
                id: EdgeId(id),
                from: (source_node, PortId::new("audio_out")),
                to: (node, PortId::new("sidechain_in")),
                role: EdgeRole::Sidechain,
            });
        }
        (None, None) => {}
    }
    if creates_edge {
        Ok(vec![NativeCreated::Edge(EdgeId(created_id.ok_or_else(
            || NativeEditError::Invalid("sidechain edge allocation failed".into()),
        )?))])
    } else {
        Ok(Vec::new())
    }
}

struct Chain {
    input: NodeId,
    strip: NodeId,
    nodes: Vec<NodeId>,
    edges: Vec<PipelineEdge>,
}

/// Returns the ordered effect identities for one conventional group.
pub(super) fn effect_chain(
    document: &NativeDocument,
    group: GroupId,
) -> Result<Vec<NodeId>, NativeEditError> {
    Ok(chain(document, group)?.nodes)
}

fn chain(document: &NativeDocument, group: GroupId) -> Result<Chain, NativeEditError> {
    let group_entry = document
        .content
        .groups
        .iter()
        .find(|entry| entry.id == group)
        .ok_or_else(|| NativeEditError::Invalid("processing group does not exist".into()))?;
    let sums: Vec<_> = document
        .content
        .processors
        .iter()
        .filter(|p| p.group == Some(group) && matches!(&p.kind, ProcessorKind::Sum))
        .collect();
    if sums.len() != 1 {
        return Err(NativeEditError::Invalid(
            "processing group must have exactly one sum input".into(),
        ));
    }
    let input = sums[0].id;
    let mut current = input;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current) {
            return Err(NativeEditError::Invalid(
                "effect chain contains a cycle".into(),
            ));
        }
        let outgoing: Vec<_> = document
            .content
            .edges
            .iter()
            .filter(|edge| {
                edge.from.0 == current
                    && edge.from.1 == PortId::new("audio_out")
                    && edge.role == EdgeRole::Main
                    && edge.to.1 == PortId::new("audio_in")
            })
            .collect();
        if outgoing.len() != 1 {
            return Err(NativeEditError::Invalid(
                "effect chain is branched or disconnected".into(),
            ));
        }
        let edge = outgoing[0];
        edges.push(edge.clone());
        if edge.to.0 == group_entry.strip {
            break;
        }
        let effect = document
            .content
            .processors
            .iter()
            .find(|p| p.id == edge.to.0)
            .ok_or_else(|| {
                NativeEditError::Invalid("effect chain points to a missing processor".into())
            })?;
        if effect.group != Some(group) || !matches!(&effect.kind, ProcessorKind::Effect { .. }) {
            return Err(NativeEditError::Invalid(
                "effect chain contains a non-effect processor".into(),
            ));
        }
        nodes.push(effect.id);
        current = effect.id;
    }
    let owned_effects: BTreeSet<_> = document
        .content
        .processors
        .iter()
        .filter_map(|processor| {
            (processor.group == Some(group)
                && matches!(&processor.kind, ProcessorKind::Effect { .. }))
            .then_some(processor.id)
        })
        .collect();
    if owned_effects != nodes.iter().copied().collect() {
        return Err(NativeEditError::Invalid(
            "processing group contains a disconnected or ambiguous effect".into(),
        ));
    }
    let chain_edges: BTreeSet<_> = edges.iter().map(|edge| edge.id).collect();
    if document.content.edges.iter().any(|edge| {
        (edge.from.0 == input || edge.to.0 == group_entry.strip) && !chain_edges.contains(&edge.id)
    }) {
        return Err(NativeEditError::Invalid(
            "insert chain has an additional input or output".into(),
        ));
    }
    for node in &nodes {
        let mut sidechains = 0;
        for edge in document.content.edges.iter().filter(|edge| {
            (edge.from.0 == *node || edge.to.0 == *node) && !chain_edges.contains(&edge.id)
        }) {
            if edge.role != EdgeRole::Sidechain
                || edge.to.0 != *node
                || edge.to.1 != PortId::new("sidechain_in")
                || edge.from.1 != PortId::new("audio_out")
                || !document.content.tracks.iter().any(|track| {
                    document
                        .content
                        .groups
                        .iter()
                        .any(|group| group.id == track.group && group.strip == edge.from.0)
                })
            {
                return Err(NativeEditError::Invalid(
                    "effect has ambiguous incident topology".into(),
                ));
            }
            sidechains += 1;
        }
        if sidechains > 1 {
            return Err(NativeEditError::Invalid(
                "effect has multiple sidechain connections".into(),
            ));
        }
    }
    Ok(Chain {
        input,
        strip: group_entry.strip,
        nodes,
        edges,
    })
}

fn strip_for_track(document: &NativeDocument, track: TrackId) -> Result<NodeId, NativeEditError> {
    let binding = document
        .content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .ok_or_else(|| NativeEditError::Invalid("sidechain source track does not exist".into()))?;
    let group = document
        .content
        .groups
        .iter()
        .find(|group| group.id == binding.group)
        .ok_or_else(|| NativeEditError::Invalid("sidechain source group does not exist".into()))?;
    Ok(group.strip)
}

fn edge_between(
    document: &NativeDocument,
    from: NodeId,
    to: NodeId,
) -> Result<&PipelineEdge, NativeEditError> {
    document
        .content
        .edges
        .iter()
        .find(|edge| edge.from.0 == from && edge.to.0 == to && edge.role == EdgeRole::Main)
        .ok_or_else(|| NativeEditError::Invalid("effect chain edge is missing".into()))
}

fn edge_between_mut(
    document: &mut NativeDocument,
    from: NodeId,
    to: NodeId,
) -> Result<&mut PipelineEdge, NativeEditError> {
    document
        .content
        .edges
        .iter_mut()
        .find(|edge| edge.from.0 == from && edge.to.0 == to && edge.role == EdgeRole::Main)
        .ok_or_else(|| NativeEditError::Invalid("effect chain edge is missing".into()))
}

fn take_id(document: &mut NativeDocument) -> Result<u64, NativeEditError> {
    let id = document.content.next_id;
    if id == 0 || id == u64::MAX {
        return Err(auris_core::pipeline::content::ContentError::IdExhausted.into());
    }
    document.content.next_id += 1;
    Ok(id)
}

fn missing_node(node: NodeId) -> NativeEditError {
    NativeEditError::Content(auris_core::pipeline::content::ContentError::Missing(
        auris_core::pipeline::content::ContentRef::Node(node),
    ))
}

fn validate_acyclic(document: &NativeDocument) -> Result<(), NativeEditError> {
    let mut indegree = BTreeMap::<NodeId, usize>::new();
    let mut outgoing = BTreeMap::<NodeId, Vec<NodeId>>::new();
    for processor in &document.content.processors {
        indegree.insert(processor.id, 0);
    }
    for edge in &document.content.edges {
        if !indegree.contains_key(&edge.from.0) || !indegree.contains_key(&edge.to.0) {
            return Err(NativeEditError::Invalid(
                "edge references a missing processor".into(),
            ));
        }
        outgoing.entry(edge.from.0).or_default().push(edge.to.0);
        *indegree.get_mut(&edge.to.0).expect("validated above") += 1;
    }
    let mut ready = VecDeque::from_iter(
        indegree
            .iter()
            .filter_map(|(node, degree)| (*degree == 0).then_some(*node)),
    );
    let mut visited = 0usize;
    while let Some(node) = ready.pop_front() {
        visited += 1;
        for next in outgoing.get(&node).into_iter().flatten() {
            let degree = indegree.get_mut(next).expect("validated above");
            *degree -= 1;
            if *degree == 0 {
                ready.push_back(*next);
            }
        }
    }
    if visited != indegree.len() {
        return Err(NativeEditError::Invalid(
            "native graph contains a cycle".into(),
        ));
    }
    Ok(())
}
