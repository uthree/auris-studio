//! Adapter from native conventional lowering to the engine-owned render input.
//!
//! This module does not expose the transient legacy `Project`. It derives the mutable command
//! targets while the native document is still available, then transfers the lowered snapshot and
//! node bindings to `auris-engine` in one operation.

use std::collections::BTreeMap;

use auris_core::TrackId;
use auris_core::pipeline::content::{CapabilityProvider, PluginParameter, ProcessorKind};
use auris_core::pipeline::document::NativeDocument;
use auris_core::pipeline::{NodeId, PipelineLimits};
use auris_engine::native::{NativeRenderError, NativeRenderInput, NativeRenderTarget};

use super::{NativeConventionalError, lower_native_conventional};

/// Failure while preparing a native document for the renderer boundary.
#[derive(Debug, thiserror::Error)]
pub enum NativePreparationError {
    /// The native graph could not be lowered to the conventional snapshot.
    #[error(transparent)]
    Lower(#[from] NativeConventionalError),
    /// The engine rejected the complete render input before taking ownership of it.
    #[error(transparent)]
    Render(#[from] NativeRenderError),
    /// A processor role could not be mapped to a conventional mutable target.
    #[error("native processor {0:?} has no conventional mutable target")]
    MissingTarget(NodeId),
    /// Runtime capability metadata was unavailable for a plugin processor.
    #[error("native plugin processor {0:?} has no capability metadata")]
    MissingCapability(NodeId),
}

/// Prepares a native conventional document for `auris-engine` without exposing a duplicate
/// Project authority to callers.
pub fn prepare_native_conventional<P: CapabilityProvider>(
    document: &NativeDocument,
    provider: &P,
    content_limits: auris_core::pipeline::content::ContentLimits,
    graph_limits: PipelineLimits,
) -> Result<NativeRenderInput, NativePreparationError> {
    let project = lower_native_conventional(document, provider, content_limits, graph_limits)?;
    let bindings = conventional_targets(document)?;
    let contracts: BTreeMap<NodeId, Vec<PluginParameter>> = document
        .content
        .processors
        .iter()
        .filter_map(|processor| match processor.kind {
            ProcessorKind::Instrument(_) | ProcessorKind::Effect { .. } => {
                Some((processor.id, processor))
            }
            _ => None,
        })
        .map(|(node, processor)| {
            let capabilities = provider
                .plugin(processor)
                .ok_or(NativePreparationError::MissingCapability(node))?;
            Ok((node, capabilities.parameters.to_vec()))
        })
        .collect::<Result<_, NativePreparationError>>()?;
    Ok(NativeRenderInput::new(project, bindings, contracts)?)
}

fn conventional_targets(
    document: &NativeDocument,
) -> Result<Vec<(NodeId, NativeRenderTarget)>, NativePreparationError> {
    let mut tracks_by_group = BTreeMap::<_, TrackId>::new();
    for binding in &document.content.tracks {
        tracks_by_group.insert(binding.group, binding.track);
    }
    let master_group = document.content.master;
    let mut targets = Vec::new();
    for processor in &document.content.processors {
        let target = match &processor.kind {
            ProcessorKind::Strip(_) => {
                if processor.id
                    == document
                        .content
                        .groups
                        .iter()
                        .find(|group| group.id == master_group)
                        .map(|group| group.strip)
                        .ok_or(NativePreparationError::MissingTarget(processor.id))?
                {
                    NativeRenderTarget::Strip(None)
                } else {
                    let track = document
                        .content
                        .groups
                        .iter()
                        .find(|group| group.strip == processor.id)
                        .and_then(|group| tracks_by_group.get(&group.id).copied())
                        .ok_or(NativePreparationError::MissingTarget(processor.id))?;
                    NativeRenderTarget::Strip(Some(track))
                }
            }
            ProcessorKind::Instrument(_) => {
                let group = document
                    .content
                    .groups
                    .iter()
                    .find(|group| group.primary_instrument == Some(processor.id))
                    .ok_or(NativePreparationError::MissingTarget(processor.id))?;
                NativeRenderTarget::Instrument(
                    *tracks_by_group
                        .get(&group.id)
                        .ok_or(NativePreparationError::MissingTarget(processor.id))?,
                )
            }
            ProcessorKind::Singer(_) => {
                let group_id = owner_group(document, processor.id)
                    .ok_or(NativePreparationError::MissingTarget(processor.id))?;
                NativeRenderTarget::Singer(
                    *tracks_by_group
                        .get(&group_id)
                        .ok_or(NativePreparationError::MissingTarget(processor.id))?,
                )
            }
            ProcessorKind::Effect { slot, .. } => {
                let group = owner_group(document, processor.id)
                    .ok_or(NativePreparationError::MissingTarget(processor.id))?;
                NativeRenderTarget::Effect {
                    track: if group == master_group {
                        None
                    } else {
                        Some(
                            *tracks_by_group
                                .get(&group)
                                .ok_or(NativePreparationError::MissingTarget(processor.id))?,
                        )
                    },
                    slot: *slot,
                }
            }
            ProcessorKind::Send { send, .. } => {
                let group = owner_group(document, processor.id)
                    .ok_or(NativePreparationError::MissingTarget(processor.id))?;
                if group == master_group {
                    return Err(NativePreparationError::MissingTarget(processor.id));
                }
                NativeRenderTarget::Send {
                    track: *tracks_by_group
                        .get(&group)
                        .ok_or(NativePreparationError::MissingTarget(processor.id))?,
                    send: *send,
                }
            }
            ProcessorKind::Source(_)
            | ProcessorKind::Automation(_)
            | ProcessorKind::Transform(_)
            | ProcessorKind::Sum
            | ProcessorKind::Output => continue,
        };
        targets.push((processor.id, target));
    }
    Ok(targets)
}

fn owner_group(
    document: &NativeDocument,
    node: NodeId,
) -> Option<auris_core::pipeline::content::GroupId> {
    document
        .content
        .processors
        .iter()
        .find(|processor| processor.id == node)
        .and_then(|processor| processor.group)
}
