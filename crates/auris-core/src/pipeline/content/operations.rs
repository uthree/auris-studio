//! Atomic native ownership edits used while preparing the M2 model.
//!
//! These operations work on an in-memory [`crate::pipeline::content::PipelineContent`] candidate. They do not
//! write a project, create an execution plan, or change the current format-32 document.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    ContentError, ContentLimits, ContentRef, GroupId, PipelineContent, ProcessorKind,
    SourceContent, SourceDocumentId,
};
use super::{ProcessingGroup, Processor};
use crate::pipeline::{EdgeId, NodeId, PipelineEdge};
use crate::project::StageId;
use crate::{ClipId, EffectSlotId, SendId};

/// Result of copying an authored source, with every new stable identity exposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCopy {
    /// New source identity.
    pub source: SourceDocumentId,
    /// Original-to-new clip identities.
    pub clips: BTreeMap<ClipId, ClipId>,
    /// Original-to-new stage identities.
    pub stages: BTreeMap<StageId, StageId>,
}

/// Result of copying one processing branch while retaining shared source nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchCopy {
    /// New processing group identity.
    pub group: GroupId,
    /// Original-to-new processor identities for owned processors.
    pub nodes: BTreeMap<NodeId, NodeId>,
    /// Original-to-new effect slot identities.
    pub effects: BTreeMap<EffectSlotId, EffectSlotId>,
    /// Original-to-new send identities.
    pub sends: BTreeMap<SendId, SendId>,
    /// Original-to-new connection identities.
    pub edges: BTreeMap<EdgeId, EdgeId>,
    /// Original-to-new control binding identities; curves remain shared.
    pub bindings: BTreeMap<EdgeId, EdgeId>,
}

impl PipelineContent {
    /// Duplicates one source and all of its authored clips with fresh identities.
    ///
    /// Notes, lyrics, recipes, curves, seeds, loop bounds and audio asset references are cloned
    /// verbatim. Existing source consumers are untouched; the returned source has no implicit
    /// timeline or processor consumer. The original content is unchanged on every error.
    pub fn duplicate_source(
        &mut self,
        source: SourceDocumentId,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<SourceCopy, ContentError> {
        self.validate_ownership(external_ids, limits)?;
        let source_entry = self
            .sources
            .iter()
            .find(|entry| entry.id == source)
            .ok_or(ContentError::Missing(ContentRef::Source(source)))?;
        let (clip_count, stage_count) = source_counts(&source_entry.content);
        ensure_increment("sources", limits.sources, self.sources.len(), 1)?;
        ensure_increment("clips", limits.clips, total_clips(self), clip_count)?;
        ensure_increment("stages", limits.stages, total_stages(self), stage_count)?;

        let allocation_count = 1 + clip_count + stage_count;
        Allocator::check_reserve(self.next_id, allocation_count)?;
        let mut candidate = self.clone();
        let mut allocator = Allocator::new(&candidate);
        let new_source = SourceDocumentId(allocator.next()?);
        let mut clip_map = BTreeMap::new();
        let mut stage_map = BTreeMap::new();
        let content = clone_source_content(
            &source_entry.content,
            &mut allocator,
            &mut clip_map,
            &mut stage_map,
        )?;
        candidate.sources.push(super::SourceDocument {
            id: new_source,
            content,
        });
        candidate.next_id = allocator.next_id;
        candidate.validate_ownership(external_ids, limits)?;
        *self = candidate;
        Ok(SourceCopy {
            source: new_source,
            clips: clip_map,
            stages: stage_map,
        })
    }

    /// Duplicates one processing group and its owned processor state.
    ///
    /// Source processors are intentionally unowned and remain shared. Incident edges are copied
    /// with fresh IDs and remapped only at owned endpoints. No track binding, master route or
    /// implicit destination is created. The original content is unchanged on every error.
    pub fn duplicate_branch(
        &mut self,
        group: GroupId,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<BranchCopy, ContentError> {
        self.validate_ownership(external_ids, limits)?;
        let original_group = self
            .groups
            .iter()
            .find(|entry| entry.id == group)
            .ok_or(ContentError::Missing(ContentRef::Group(group)))?;
        if group == self.master {
            return Err(ContentError::Role(
                ContentRef::Group(group),
                "the master group is not a branch",
            ));
        }
        let owned: BTreeSet<NodeId> = self
            .processors
            .iter()
            .filter(|node| node.group == Some(group))
            .map(|node| node.id)
            .collect();
        if !owned.contains(&original_group.strip) {
            return Err(ContentError::Role(
                ContentRef::Group(group),
                "group strip is not owned",
            ));
        }
        ensure_increment("groups", limits.groups, self.groups.len(), 1)?;
        ensure_increment(
            "processors",
            limits.processors,
            self.processors.len(),
            owned.len(),
        )?;
        let incident_edges = self
            .edges
            .iter()
            .filter(|edge| owned.contains(&edge.from.0) || owned.contains(&edge.to.0))
            .count();
        ensure_increment("edges", limits.edges, self.edges.len(), incident_edges)?;
        let binding_count = self
            .automation_bindings
            .iter()
            .filter(|binding| owned.contains(&binding.target.node))
            .count();
        ensure_increment(
            "bindings",
            limits.bindings,
            self.automation_bindings.len(),
            binding_count,
        )?;

        let effect_count = self
            .processors
            .iter()
            .filter(|node| {
                owned.contains(&node.id) && matches!(&node.kind, ProcessorKind::Effect { .. })
            })
            .count();
        let send_count = self
            .processors
            .iter()
            .filter(|node| {
                owned.contains(&node.id) && matches!(&node.kind, ProcessorKind::Send { .. })
            })
            .count();
        let allocation_count =
            1 + owned.len() + effect_count + send_count + incident_edges + binding_count;
        Allocator::check_reserve(self.next_id, allocation_count)?;
        let mut candidate = self.clone();
        let mut allocator = Allocator::new(&candidate);
        let new_group = GroupId(allocator.next()?);
        let mut node_map = BTreeMap::new();
        let mut effect_map = BTreeMap::new();
        let mut send_map = BTreeMap::new();
        let mut new_processors = Vec::with_capacity(owned.len());
        for processor in self
            .processors
            .iter()
            .filter(|node| owned.contains(&node.id))
        {
            let new_id = NodeId(allocator.next()?);
            node_map.insert(processor.id, new_id);
            let mut kind = processor.kind.clone();
            match &mut kind {
                ProcessorKind::Effect { slot, .. } => {
                    let new_slot = EffectSlotId(allocator.next()?);
                    effect_map.insert(*slot, new_slot);
                    *slot = new_slot;
                }
                ProcessorKind::Send { send, .. } => {
                    let new_send = SendId(allocator.next()?);
                    send_map.insert(*send, new_send);
                    *send = new_send;
                }
                _ => {}
            }
            new_processors.push(Processor {
                id: new_id,
                group: Some(new_group),
                kind,
            });
        }
        let new_strip = *node_map
            .get(&original_group.strip)
            .ok_or(ContentError::Role(
                ContentRef::Group(group),
                "group strip is missing from owned processors",
            ))?;
        let new_primary = original_group
            .primary_instrument
            .map(|id| {
                node_map.get(&id).copied().ok_or(ContentError::Role(
                    ContentRef::Group(group),
                    "primary instrument is not owned by group",
                ))
            })
            .transpose()?;
        let mut edge_map = BTreeMap::new();
        let mut new_edges = Vec::with_capacity(incident_edges);
        for edge in self
            .edges
            .iter()
            .filter(|edge| owned.contains(&edge.from.0) || owned.contains(&edge.to.0))
        {
            let id = EdgeId(allocator.next()?);
            edge_map.insert(edge.id, id);
            let from = node_map.get(&edge.from.0).copied().unwrap_or(edge.from.0);
            let to = node_map.get(&edge.to.0).copied().unwrap_or(edge.to.0);
            new_edges.push(PipelineEdge {
                id,
                from: (from, edge.from.1.clone()),
                to: (to, edge.to.1.clone()),
                role: edge.role,
            });
        }
        candidate.groups.push(ProcessingGroup {
            id: new_group,
            strip: new_strip,
            primary_instrument: new_primary,
        });
        candidate.processors.extend(new_processors);
        candidate.edges.extend(new_edges);
        let mut binding_map = BTreeMap::new();
        for binding in self
            .automation_bindings
            .iter()
            .filter(|binding| owned.contains(&binding.target.node))
        {
            let mut copied = binding.clone();
            copied.id = EdgeId(allocator.next()?);
            copied.target.node = *node_map
                .get(&binding.target.node)
                .ok_or(ContentError::Missing(ContentRef::Node(binding.target.node)))?;
            binding_map.insert(binding.id, copied.id);
            candidate.automation_bindings.push(copied);
        }
        candidate.next_id = allocator.next_id;
        candidate.validate_ownership(external_ids, limits)?;
        *self = candidate;
        Ok(BranchCopy {
            group: new_group,
            nodes: node_map,
            effects: effect_map,
            sends: send_map,
            edges: edge_map,
            bindings: binding_map,
        })
    }

    /// Removes a non-master group and all of its owned processors and incident edges atomically.
    pub fn remove_group(
        &mut self,
        group: GroupId,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<bool, ContentError> {
        self.validate_ownership(external_ids, limits)?;
        if group == self.master {
            return Err(ContentError::MasterRemoval);
        }
        if !self.groups.iter().any(|entry| entry.id == group) {
            return Err(ContentError::Missing(ContentRef::Group(group)));
        }
        let owned: BTreeSet<_> = self
            .processors
            .iter()
            .filter(|node| node.group == Some(group))
            .map(|node| node.id)
            .collect();
        let mut candidate = self.clone();
        candidate.groups.retain(|entry| entry.id != group);
        candidate
            .processors
            .retain(|node| node.group != Some(group));
        candidate
            .edges
            .retain(|edge| !owned.contains(&edge.from.0) && !owned.contains(&edge.to.0));
        candidate.tracks.retain(|track| track.group != group);
        candidate
            .automation_bindings
            .retain(|binding| !owned.contains(&binding.target.node));
        candidate.validate_ownership(external_ids, limits)?;
        *self = candidate;
        Ok(true)
    }

    /// Removes a source only when no timeline or source processor still consumes it.
    pub fn remove_source(
        &mut self,
        source: SourceDocumentId,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<bool, ContentError> {
        self.validate_ownership(external_ids, limits)?;
        let consumers = self.source_consumers(source)?;
        if !consumers.is_empty() {
            return Err(ContentError::SourceInUse(source, consumers));
        }
        let mut candidate = self.clone();
        candidate.sources.retain(|entry| entry.id != source);
        candidate.validate_ownership(external_ids, limits)?;
        *self = candidate;
        Ok(true)
    }

    /// Copies a curve with a fresh identity; source nodes and bindings are not copied implicitly.
    pub fn duplicate_automation(
        &mut self,
        id: super::AutomationId,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<super::AutomationId, ContentError> {
        self.validate_ownership(external_ids, limits)?;
        let original = self
            .automation
            .iter()
            .find(|curve| curve.id == id)
            .ok_or(ContentError::Missing(ContentRef::Automation(id)))?;
        ensure_increment("automations", limits.automations, self.automation.len(), 1)?;
        let points = self.automation.iter().fold(0usize, |sum, curve| {
            sum.saturating_add(curve.points().len())
        });
        ensure_increment(
            "automation_points",
            limits.automation_points,
            points,
            original.points().len(),
        )?;
        Allocator::check_reserve(self.next_id, 1)?;
        let mut candidate = self.clone();
        let mut copied = original.clone();
        copied.id = super::AutomationId(candidate.next_id);
        candidate.next_id += 1;
        let result = copied.id;
        candidate.automation.push(copied);
        candidate.validate_ownership(external_ids, limits)?;
        *self = candidate;
        Ok(result)
    }

    /// Removes a curve only after all of its source nodes have been detached.
    pub fn remove_automation(
        &mut self,
        id: super::AutomationId,
        external_ids: &[u64],
        limits: ContentLimits,
    ) -> Result<(), ContentError> {
        self.validate_ownership(external_ids, limits)?;
        if !self.automation.iter().any(|curve| curve.id == id) {
            return Err(ContentError::Missing(ContentRef::Automation(id)));
        }
        let consumers: Vec<_> = self
            .processors
            .iter()
            .filter(|node| matches!(node.kind, ProcessorKind::Automation(curve) if curve == id))
            .map(|node| ContentRef::Node(node.id))
            .collect();
        if !consumers.is_empty() {
            return Err(ContentError::AutomationInUse(id, consumers));
        }
        self.automation.retain(|curve| curve.id != id);
        Ok(())
    }
}

fn ensure_increment(
    name: &'static str,
    limit: usize,
    current: usize,
    amount: usize,
) -> Result<(), ContentError> {
    if amount > limit.saturating_sub(current) {
        Err(ContentError::Limit(name))
    } else {
        Ok(())
    }
}

fn total_clips(content: &PipelineContent) -> usize {
    content
        .sources
        .iter()
        .map(|source| source_counts(&source.content).0)
        .sum()
}
fn total_stages(content: &PipelineContent) -> usize {
    content
        .sources
        .iter()
        .map(|source| source_counts(&source.content).1)
        .sum()
}

fn source_counts(content: &SourceContent) -> (usize, usize) {
    match content {
        SourceContent::Notes(clips) => (
            clips.len(),
            clips.iter().map(|clip| clip.transforms.len()).sum(),
        ),
        SourceContent::Audio(clips) => (clips.len(), 0),
    }
}

fn clone_source_content(
    content: &SourceContent,
    allocator: &mut Allocator,
    clip_map: &mut BTreeMap<ClipId, ClipId>,
    stage_map: &mut BTreeMap<StageId, StageId>,
) -> Result<SourceContent, ContentError> {
    match content {
        SourceContent::Audio(clips) => {
            let mut copied = Vec::with_capacity(clips.len());
            for clip in clips {
                let old = clip.id;
                let new = ClipId(allocator.next()?);
                let mut copied_clip = clip.clone();
                copied_clip.id = new;
                clip_map.insert(old, new);
                copied.push(copied_clip);
            }
            Ok(SourceContent::Audio(copied))
        }
        SourceContent::Notes(clips) => {
            let mut copied = Vec::with_capacity(clips.len());
            for clip in clips {
                let old_clip = clip.id;
                let new_clip = ClipId(allocator.next()?);
                let mut new_clip_data = clip.clone();
                new_clip_data.id = new_clip;
                clip_map.insert(old_clip, new_clip);
                for stage in &mut new_clip_data.transforms {
                    let old = stage.id;
                    let new = StageId(allocator.next()?);
                    stage.id = new;
                    stage_map.insert(old, new);
                }
                copied.push(new_clip_data);
            }
            Ok(SourceContent::Notes(copied))
        }
    }
}

struct Allocator {
    next_id: u64,
}
impl Allocator {
    fn new(content: &PipelineContent) -> Self {
        Self {
            next_id: content.next_id,
        }
    }
    fn check_reserve(next_id: u64, count: usize) -> Result<(), ContentError> {
        let count = u64::try_from(count).map_err(|_| ContentError::IdExhausted)?;
        next_id
            .checked_add(count)
            .ok_or(ContentError::IdExhausted)
            .map(|_| ())
    }
    fn next(&mut self) -> Result<u64, ContentError> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(ContentError::IdExhausted)?;
        Ok(id)
    }
}
