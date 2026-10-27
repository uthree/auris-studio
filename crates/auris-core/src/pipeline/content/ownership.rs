use std::collections::{BTreeMap, BTreeSet};

use super::{
    ContentError, ContentLimits, ContentRef, PipelineContent, ProcessorKind, SourceContent,
};

pub(super) fn validate(
    content: &PipelineContent,
    external_ids: &[u64],
    limits: ContentLimits,
) -> Result<(), ContentError> {
    let highest = inspect(content, external_ids, limits)?;
    if content.next_id <= highest {
        return Err(ContentError::StaleCounter(highest));
    }
    Ok(())
}

pub(super) fn repair(
    content: &mut PipelineContent,
    external_ids: &[u64],
    limits: ContentLimits,
) -> Result<(), ContentError> {
    let highest = inspect(content, external_ids, limits)?;
    let after = highest.checked_add(1).ok_or(ContentError::IdExhausted)?;
    content.next_id = content.next_id.max(after);
    Ok(())
}

fn bound(name: &'static str, count: usize, limit: usize) -> Result<(), ContentError> {
    if count > limit {
        Err(ContentError::Limit(name))
    } else {
        Ok(())
    }
}

fn preflight(
    content: &PipelineContent,
    external_ids: &[u64],
    limits: ContentLimits,
) -> Result<(), ContentError> {
    bound("sources", content.sources.len(), limits.sources)?;
    bound("processors", content.processors.len(), limits.processors)?;
    bound("groups", content.groups.len(), limits.groups)?;
    bound("tracks", content.tracks.len(), limits.tracks)?;
    bound("edges", content.edges.len(), limits.edges)?;
    bound("external_ids", external_ids.len(), limits.external_ids)?;
    bound("automations", content.automation.len(), limits.automations)?;
    bound(
        "bindings",
        content.automation_bindings.len(),
        limits.bindings,
    )?;
    let points = content.automation.iter().fold(0usize, |count, curve| {
        count.saturating_add(curve.points().len())
    });
    bound("automation_points", points, limits.automation_points)?;
    let mut clips = 0usize;
    let mut stages = 0usize;
    for source in &content.sources {
        let count = match &source.content {
            SourceContent::Notes(notes) => notes.len(),
            SourceContent::Audio(audio) => audio.len(),
        };
        clips = clips.saturating_add(count);
        bound("clips", clips, limits.clips)?;
    }
    for source in &content.sources {
        if let SourceContent::Notes(clips) = &source.content {
            for clip in clips {
                stages = stages.saturating_add(clip.transforms.len());
            }
        }
    }
    bound("stages", stages, limits.stages)
}

fn register(ids: &mut BTreeSet<u64>, id: u64) -> Result<(), ContentError> {
    if ids.insert(id) {
        Ok(())
    } else {
        Err(ContentError::DuplicateId(id))
    }
}

fn inspect(
    content: &PipelineContent,
    external_ids: &[u64],
    limits: ContentLimits,
) -> Result<u64, ContentError> {
    preflight(content, external_ids, limits)?;
    let mut ids = BTreeSet::new();
    for &id in external_ids {
        register(&mut ids, id)?;
    }
    let mut sources = BTreeSet::new();
    let mut automation = BTreeSet::new();
    for curve in &content.automation {
        register(&mut ids, curve.id.0)?;
        curve.validate()?;
        automation.insert(curve.id);
    }
    for source in &content.sources {
        register(&mut ids, source.id.0)?;
        sources.insert(source.id);
        match &source.content {
            SourceContent::Notes(clips) => {
                for clip in clips {
                    register(&mut ids, clip.id.0)?;
                    clip.validate_stage_ids()?;
                    for stage in &clip.transforms {
                        register(&mut ids, stage.id.0)?;
                    }
                }
            }
            SourceContent::Audio(clips) => {
                for clip in clips {
                    register(&mut ids, clip.id.0)?;
                }
            }
        }
    }
    let mut groups = BTreeMap::new();
    for group in &content.groups {
        register(&mut ids, group.id.0)?;
        groups.insert(group.id, group);
    }
    if !groups.contains_key(&content.master) {
        return Err(ContentError::Missing(ContentRef::Group(content.master)));
    }
    let mut processors = BTreeMap::new();
    for node in &content.processors {
        register(&mut ids, node.id.0)?;
        processors.insert(node.id, node);
        match node.kind {
            ProcessorKind::Automation(curve) => {
                if node.group.is_some() {
                    return Err(ContentError::Role(
                        ContentRef::Node(node.id),
                        "automation sources must be independent of group ownership",
                    ));
                }
                if !automation.contains(&curve) {
                    return Err(ContentError::Missing(ContentRef::Automation(curve)));
                }
            }
            ProcessorKind::Source(source) => {
                if node.group.is_some() {
                    return Err(ContentError::Role(
                        ContentRef::Node(node.id),
                        "source nodes must be independent of group ownership",
                    ));
                }
                if !sources.contains(&source) {
                    return Err(ContentError::Missing(ContentRef::Source(source)));
                }
            }
            _ => {
                let group = node.group.ok_or(ContentError::Role(
                    ContentRef::Node(node.id),
                    "processor must have an owning group",
                ))?;
                if !groups.contains_key(&group) {
                    return Err(ContentError::Missing(ContentRef::Group(group)));
                }
            }
        }
        match node.kind {
            ProcessorKind::Effect { slot, .. } => register(&mut ids, slot.0)?,
            ProcessorKind::Send { send, .. } => register(&mut ids, send.0)?,
            _ => {}
        }
    }
    for group in &content.groups {
        let strip = processors
            .get(&group.strip)
            .ok_or(ContentError::Missing(ContentRef::Node(group.strip)))?;
        if strip.group != Some(group.id) || !matches!(strip.kind, ProcessorKind::Strip(_)) {
            return Err(ContentError::Role(
                ContentRef::Group(group.id),
                "console strip must be an owned strip node",
            ));
        }
        if let Some(primary) = group.primary_instrument {
            let node = processors
                .get(&primary)
                .ok_or(ContentError::Missing(ContentRef::Node(primary)))?;
            if node.group != Some(group.id) || !matches!(node.kind, ProcessorKind::Instrument(_)) {
                return Err(ContentError::Role(
                    ContentRef::Group(group.id),
                    "primary instrument must be an owned instrument node",
                ));
            }
        }
    }
    for track in &content.tracks {
        register(&mut ids, track.track.0)?;
        if !groups.contains_key(&track.group) {
            return Err(ContentError::Missing(ContentRef::Group(track.group)));
        }
        if track.group == content.master {
            return Err(ContentError::Role(
                ContentRef::Track(track.track),
                "timeline track cannot use the master as its home group",
            ));
        }
        if let Some(source) = track.source
            && !sources.contains(&source)
        {
            return Err(ContentError::Missing(ContentRef::Source(source)));
        }
    }
    for edge in &content.edges {
        register(&mut ids, edge.id.0)?;
        if edge.role == crate::pipeline::EdgeRole::Control {
            return Err(ContentError::Role(
                ContentRef::Edge(edge.id),
                "control connections must use automation bindings",
            ));
        }
        for node in [edge.from.0, edge.to.0] {
            if !processors.contains_key(&node) {
                return Err(ContentError::Missing(ContentRef::Node(node)));
            }
        }
    }
    for binding in &content.automation_bindings {
        register(&mut ids, binding.id.0)?;
        let source = processors
            .get(&binding.source)
            .ok_or(ContentError::Missing(ContentRef::Node(binding.source)))?;
        if !matches!(source.kind, ProcessorKind::Automation(_)) {
            return Err(ContentError::Role(
                ContentRef::Edge(binding.id),
                "binding source must be an automation processor",
            ));
        }
        if !processors.contains_key(&binding.target.node) {
            return Err(ContentError::Missing(ContentRef::Node(binding.target.node)));
        }
        if binding.target.key.is_empty() {
            return Err(ContentError::ParameterKeyRequired);
        }
    }
    Ok(ids.last().copied().unwrap_or(0))
}
