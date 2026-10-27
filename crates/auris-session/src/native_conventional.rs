//! Bounded M2 lowering from the native pipeline document to the existing renderer snapshot.
//!
//! This is deliberately a compatibility bridge, not a second persisted model. It accepts only
//! the conventional one-source/one-chain shapes that the current `RenderGraph` can execute.

use std::collections::{BTreeMap, BTreeSet};

use auris_core::ParamTarget;
use auris_core::automation::Automation;
use auris_core::pipeline::content::{
    CapabilityProvider, PipelineContent, Processor, ProcessorKind, SourceContent,
};
use auris_core::pipeline::document::{NativeDocument, NativeTrackKind};
use auris_core::pipeline::{EdgeRole, NodeId, PipelineLimits, PortId};
use auris_core::project::{
    AudioTrack, AuxSend, EffectSlot, InstrumentTrack, MixerStrip, Output, Project, SingerTrack,
    Track, TrackId, TrackKind,
};

mod preparation;
#[cfg(test)]
mod preparation_tests;
#[cfg(test)]
mod shape_tests;
#[cfg(test)]
mod tests;

pub use preparation::{NativePreparationError, prepare_native_conventional};

/// Failure while lowering a native document into the conventional renderer snapshot.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NativeConventionalError {
    /// Native ownership or capability validation failed.
    #[error("native document validation failed: {0}")]
    Validation(String),
    /// The graph is valid but not representable by the conventional renderer.
    #[error("native graph is not conventional: {0}")]
    Unsupported(&'static str),
    /// A required native object was absent.
    #[error("native conventional lowering is missing {0}")]
    Missing(String),
    /// A stable plugin parameter key could not be mapped to a runtime parameter id.
    #[error("native automation parameter key {key:?} is unavailable on {node:?}")]
    ParameterKey {
        /// Processor owning the missing key.
        node: NodeId,
        /// Stable key requested by the native binding.
        key: String,
    },
}

/// Lowers a validated conventional native document into an ephemeral legacy renderer snapshot.
///
/// The returned [`Project`] is intended only as transient renderer input; it does not preserve
/// native identity/topology and must not be passed to a save path. Its private allocation counter
/// is not an authority. Every native node and signal edge must be consumed by this lowering;
/// unsupported branches fail explicitly instead of being silently discarded.
pub(crate) fn lower_native_conventional<P: CapabilityProvider>(
    document: &NativeDocument,
    provider: &P,
    content_limits: auris_core::pipeline::content::ContentLimits,
    graph_limits: PipelineLimits,
) -> Result<Project, NativeConventionalError> {
    document
        .validate_ownership(content_limits)
        .map_err(|error| NativeConventionalError::Validation(error.to_string()))?;
    let external_ids = document.external_ids();
    let resolved = document
        .content
        .resolve_pipeline(&external_ids, content_limits, graph_limits, provider)
        .map_err(|error| NativeConventionalError::Validation(error.to_string()))?;

    for binding in document
        .content
        .automation_bindings
        .iter()
        .filter(|binding| binding.enabled)
    {
        let parameter = resolved
            .pipeline()
            .nodes
            .iter()
            .find(|node| node.id == binding.target.node)
            .and_then(|node| {
                node.parameters
                    .iter()
                    .find(|parameter| parameter.key == binding.target.key)
            })
            .ok_or_else(|| NativeConventionalError::ParameterKey {
                node: binding.target.node,
                key: binding.target.key.clone(),
            })?;
        if parameter.policy != auris_core::pipeline::AutomationPolicy::Continuous {
            return Err(NativeConventionalError::Unsupported(
                "conventional automation requires continuous parameter control",
            ));
        }
    }

    let processors: BTreeMap<NodeId, &Processor> = document
        .content
        .processors
        .iter()
        .map(|processor| (processor.id, processor))
        .collect();
    for processor in processors.values() {
        if let Some(metadata) = provider.plugin(processor)
            && metadata
                .parameters
                .iter()
                .enumerate()
                .any(|(index, parameter)| parameter.descriptor.id.index() != index)
        {
            return Err(NativeConventionalError::Unsupported(
                "plugin parameter IDs must match runtime positions",
            ));
        }
    }
    if document
        .content
        .processors
        .iter()
        .any(|processor| matches!(&processor.kind, ProcessorKind::Transform(_)))
    {
        return Err(NativeConventionalError::Unsupported(
            "explicit transform processors are reserved for M3",
        ));
    }

    let mut used_edges = BTreeSet::new();
    let mut tracks = Vec::with_capacity(document.track_presentation.len());
    let mut effect_slots = BTreeMap::new();
    let mut send_nodes = BTreeMap::new();
    let mut input_nodes = BTreeMap::new();
    let mut strip_nodes = BTreeMap::new();
    let mut source_nodes = BTreeMap::new();
    let mut instrument_nodes = BTreeMap::new();
    let mut singer_nodes = BTreeMap::new();
    let mut seen_sources = BTreeSet::new();

    for presentation in &document.track_presentation {
        let binding = document
            .content
            .tracks
            .iter()
            .find(|binding| binding.track == presentation.id)
            .ok_or_else(|| {
                NativeConventionalError::Missing(format!("track {:?}", presentation.id))
            })?;
        let group = document
            .content
            .groups
            .iter()
            .find(|group| group.id == binding.group)
            .ok_or_else(|| {
                NativeConventionalError::Missing(format!("group {:?}", binding.group))
            })?;
        let strip = processor(&processors, group.strip)?;
        let ProcessorKind::Strip(strip_state) = &strip.kind else {
            return Err(NativeConventionalError::Unsupported(
                "group strip is not a strip",
            ));
        };
        let input = find_group_input(&document.content, group.id)?;
        input_nodes.insert(presentation.id, input.id);
        strip_nodes.insert(presentation.id, strip.id);

        let source = binding
            .source
            .map(|source_id| {
                document
                    .content
                    .sources
                    .iter()
                    .find(|source| source.id == source_id)
                    .ok_or_else(|| {
                        NativeConventionalError::Missing(format!("source {source_id:?}"))
                    })
            })
            .transpose()?;
        if let Some(source) = source {
            if !seen_sources.insert(source.id) {
                return Err(NativeConventionalError::Unsupported(
                    "shared source documents are not conventional",
                ));
            }
            let source_node = document
                .content
                .processors
                .iter()
                .find_map(|processor| {
                    matches!(&processor.kind, ProcessorKind::Source(id) if *id == source.id)
                        .then_some(processor.id)
                })
                .ok_or_else(|| {
                    NativeConventionalError::Missing(format!(
                        "source processor for {:?}",
                        source.id
                    ))
                })?;
            source_nodes.insert(presentation.id, source_node);
        }

        let primary = group
            .primary_instrument
            .map(|node| processor(&processors, node).map(|processor| (node, processor)))
            .transpose()?;
        if let Some((node, _processor)) = primary {
            instrument_nodes.insert(presentation.id, node);
        }
        let mut singers = document.content.processors.iter().filter(|processor| {
            processor.group == Some(group.id) && matches!(&processor.kind, ProcessorKind::Singer(_))
        });
        let singer = singers.next();
        if (presentation.kind == NativeTrackKind::Singer) != singer.is_some()
            || singers.next().is_some()
        {
            return Err(NativeConventionalError::Unsupported(
                "a conventional singer processor requires exactly one singer presentation",
            ));
        }
        if let Some(singer) = singer {
            singer_nodes.insert(presentation.id, singer.id);
        }

        for processor in document
            .content
            .processors
            .iter()
            .filter(|processor| processor.group == Some(group.id))
        {
            match &processor.kind {
                ProcessorKind::Effect { slot, .. } => {
                    effect_slots.insert(*slot, processor.id);
                }
                ProcessorKind::Send { send, .. } => {
                    send_nodes.insert(*send, processor.id);
                }
                _ => {}
            }
        }

        if let Some(source) = source {
            let source_node = source_nodes
                .get(&presentation.id)
                .copied()
                .ok_or_else(|| NativeConventionalError::Missing("source node".into()))?;
            if matches!(&source.content, SourceContent::Notes(_)) {
                consume_source_edges(
                    &resolved,
                    source_node,
                    primary.map(|entry| entry.0),
                    singer_nodes.get(&presentation.id).copied(),
                    input.id,
                    &mut used_edges,
                )?;
            } else {
                let audio = resolved
                    .pipeline()
                    .edges
                    .iter()
                    .find(|edge| {
                        edge.from.0 == source_node
                            && edge.from.1 == PortId::new("audio_out")
                            && edge.to.0 == input.id
                            && edge.to.1 == PortId::new("audio_in")
                            && edge.role == EdgeRole::Main
                    })
                    .ok_or_else(|| NativeConventionalError::Missing("audio source edge".into()))?;
                used_edges.insert(audio.id);
            }
        }
        let kind = match presentation.kind {
            NativeTrackKind::Bus => TrackKind::Bus,
            NativeTrackKind::Audio => {
                let source = source.ok_or_else(|| {
                    NativeConventionalError::Missing(format!(
                        "audio source for track {:?}",
                        presentation.id
                    ))
                })?;
                let SourceContent::Audio(clips) = &source.content else {
                    return Err(NativeConventionalError::Unsupported(
                        "audio track does not own an audio source",
                    ));
                };
                TrackKind::Audio(AudioTrack {
                    clips: clips.clone(),
                })
            }
            NativeTrackKind::Instrument | NativeTrackKind::Drum => {
                let source = source.ok_or_else(|| {
                    NativeConventionalError::Missing(format!(
                        "note source for track {:?}",
                        presentation.id
                    ))
                })?;
                let SourceContent::Notes(clips) = &source.content else {
                    return Err(NativeConventionalError::Unsupported(
                        "instrument track does not own notes",
                    ));
                };
                let instrument_node = primary
                    .ok_or_else(|| {
                        NativeConventionalError::Missing(format!(
                            "instrument for track {:?}",
                            presentation.id
                        ))
                    })?
                    .0;
                let ProcessorKind::Instrument(instance) =
                    &processor(&processors, instrument_node)?.kind
                else {
                    return Err(NativeConventionalError::Unsupported(
                        "track primary is not an instrument",
                    ));
                };
                let clips = clips.iter().map(|clip| clip.performer_clip()).collect();
                let instrument = InstrumentTrack {
                    instrument_id: instance.plugin_id.clone(),
                    instrument_state: instance.state.clone(),
                    clips,
                    file: instance.file.clone(),
                };
                if presentation.kind == NativeTrackKind::Drum {
                    TrackKind::Drum(instrument)
                } else {
                    TrackKind::Instrument(instrument)
                }
            }
            NativeTrackKind::Singer => {
                let source = source.ok_or_else(|| {
                    NativeConventionalError::Missing(format!(
                        "singer source for track {:?}",
                        presentation.id
                    ))
                })?;
                let SourceContent::Notes(clips) = &source.content else {
                    return Err(NativeConventionalError::Unsupported(
                        "singer track does not own notes",
                    ));
                };
                let guide_node = group.primary_instrument.ok_or_else(|| {
                    NativeConventionalError::Missing(format!(
                        "singer guide for track {:?}",
                        presentation.id
                    ))
                })?;
                let singer_node = singer_nodes.get(&presentation.id).copied().ok_or_else(|| {
                    NativeConventionalError::Missing(format!(
                        "singer processor for track {:?}",
                        presentation.id
                    ))
                })?;
                let ProcessorKind::Instrument(guide) = &processor(&processors, guide_node)?.kind
                else {
                    return Err(NativeConventionalError::Unsupported(
                        "singer guide is not an instrument",
                    ));
                };
                if guide.file.is_some() {
                    return Err(NativeConventionalError::Unsupported(
                        "hosted singer guides are not representable by SingerTrack",
                    ));
                }
                let ProcessorKind::Singer(instance) = &processor(&processors, singer_node)?.kind
                else {
                    return Err(NativeConventionalError::Unsupported(
                        "singer primary is not a singer",
                    ));
                };
                TrackKind::Singer(SingerTrack {
                    instrument_id: guide.plugin_id.clone(),
                    instrument_state: guide.state.clone(),
                    clips: clips.iter().map(|clip| clip.performer_clip()).collect(),
                    frame_hop: instance.frame_hop,
                    voice: instance.voice.clone(),
                    take: instance.take.clone(),
                })
            }
        };

        let effects = ordered_effects(
            &document.content,
            &resolved,
            input.id,
            strip.id,
            &effect_slots,
            &mut used_edges,
        )?;
        let sends = ordered_sends(
            &document.content,
            &resolved,
            strip.id,
            &send_nodes,
            &mut used_edges,
        )?;
        let mut mixer = mixer_from_state(strip_state);
        mixer.effects = effects;
        tracks.push(Track {
            id: presentation.id,
            name: presentation.name.clone(),
            color: presentation.color,
            height: presentation.height,
            kind,
            mixer,
            outputs: Vec::new(),
            sends,
        });
    }

    // The remaining signal edges are checked while reconstructing conventional outputs/master.
    // This phase is intentionally separate so every track is present before bus destinations are
    // translated back to the legacy route vocabulary.
    if !document.context.sample_rate.is_finite() || document.context.sample_rate <= 0.0 {
        return Err(NativeConventionalError::Unsupported(
            "native sample rate is invalid",
        ));
    }
    let mut project = Project::new(document.context.name.clone(), document.context.sample_rate);
    project.format_version = auris_core::Project::FORMAT_VERSION;
    project.saved_by = document.context.saved_by.clone();
    project.tempo_map = document.context.tempo_map.clone();
    project.signatures = document.context.signatures.clone();
    project.harmony = document.context.harmony.clone();
    project.sections = document.context.sections.clone();
    project.audio_sources = document.context.audio_sources.clone();
    project.soundfonts = document.context.soundfonts.clone();
    project.loop_region = document.context.loop_region;
    project.loop_enabled = document.context.loop_enabled;
    project.punch_region = document.context.punch_region;
    project.punch_enabled = document.context.punch_enabled;
    project.metronome = document.context.metronome;
    project.count_in_bars = document.context.count_in_bars;
    project.grid = document.context.grid;
    project.song_spec = document.context.song_spec.clone();
    project.creation_brief = document.context.creation_brief.clone();
    project.composition_baseline = document.context.composition_baseline.clone();
    project.transcription_provenance = document.context.transcription_provenance.clone();
    project.mixer_graph_view = document.context.mixer_graph_view.clone();

    let master_group = document
        .content
        .groups
        .iter()
        .find(|group| group.id == document.content.master)
        .ok_or_else(|| NativeConventionalError::Missing("master group".into()))?;
    let master_strip = processor(&processors, master_group.strip)?;
    let ProcessorKind::Strip(master_state) = &master_strip.kind else {
        return Err(NativeConventionalError::Unsupported(
            "master group strip is not a strip",
        ));
    };
    project.master = mixer_from_state(master_state);
    for processor in document
        .content
        .processors
        .iter()
        .filter(|processor| processor.group == Some(document.content.master))
    {
        if let ProcessorKind::Effect { slot, .. } = &processor.kind {
            effect_slots.insert(*slot, processor.id);
        }
    }
    let master_input = find_group_input(&document.content, document.content.master)?.id;
    project.master.effects = ordered_effects(
        &document.content,
        &resolved,
        master_input,
        master_strip.id,
        &effect_slots,
        &mut used_edges,
    )?;
    project.tracks = tracks;
    consume_master_output_edges(
        &document.content,
        &resolved,
        master_strip.id,
        &mut used_edges,
    )?;
    lower_routes(
        &document.content,
        &resolved,
        &mut project,
        &input_nodes,
        &strip_nodes,
        &mut used_edges,
    )?;
    lower_automation(
        &document.content,
        &project.tracks,
        provider,
        &processors,
        AutomationTargets {
            strips: &strip_nodes,
            master_strip: master_strip.id,
            effect_slots: &effect_slots,
            instruments: &instrument_nodes,
        },
        &mut project.automation,
    )?;
    if used_edges.len() != document.content.edges.len() {
        return Err(NativeConventionalError::Unsupported(
            "unconsumed native signal edge",
        ));
    }
    let consumed_effects = project
        .tracks
        .iter()
        .flat_map(|track| track.mixer.effects.iter())
        .chain(project.master.effects.iter())
        .filter_map(|effect| effect_slots.get(&effect.id).copied());
    let mut consumed_nodes: BTreeSet<NodeId> = input_nodes
        .values()
        .copied()
        .chain(strip_nodes.values().copied())
        .chain(source_nodes.values().copied())
        .chain(instrument_nodes.values().copied())
        .chain(singer_nodes.values().copied())
        .chain(consumed_effects)
        .chain(send_nodes.values().copied())
        .collect();
    // Unbound and disabled-only curves have no audible contribution. Keep their authored
    // state in the native document; this ephemeral snapshot includes active drivers only.
    consumed_nodes.extend(document.content.processors.iter().filter_map(|processor| {
        matches!(&processor.kind, ProcessorKind::Automation(_)).then_some(processor.id)
    }));
    consumed_nodes.insert(master_strip.id);
    consumed_nodes.insert(master_input);
    consumed_nodes.extend(
        resolved
            .pipeline()
            .edges
            .iter()
            .filter(|edge| used_edges.contains(&edge.id))
            .filter_map(|edge| {
                document
                    .content
                    .processors
                    .iter()
                    .find(|processor| {
                        processor.id == edge.to.0
                            && matches!(&processor.kind, ProcessorKind::Output)
                    })
                    .map(|processor| processor.id)
            }),
    );
    if document
        .content
        .processors
        .iter()
        .any(|processor| !consumed_nodes.contains(&processor.id))
    {
        return Err(NativeConventionalError::Unsupported(
            "unconsumed native processor",
        ));
    }
    if !project.repair_id_counter() || !project.routing_is_valid() {
        return Err(NativeConventionalError::Unsupported(
            "lowered routes or identities are not conventional",
        ));
    }
    project
        .validate_loop_expansion()
        .map_err(|error| NativeConventionalError::Validation(error.to_string()))?;
    Ok(project)
}

fn consume_source_edges(
    resolved: &auris_core::pipeline::content::ResolvedContent<'_>,
    source: NodeId,
    primary: Option<NodeId>,
    singer: Option<NodeId>,
    input: NodeId,
    used: &mut BTreeSet<auris_core::pipeline::EdgeId>,
) -> Result<(), NativeConventionalError> {
    let destination = primary.ok_or(NativeConventionalError::Unsupported(
        "source has no performer",
    ))?;
    let event = resolved
        .pipeline()
        .edges
        .iter()
        .find(|edge| {
            edge.from.0 == source
                && edge.from.1 == PortId::new("events_out")
                && edge.to.0 == destination
                && edge.to.1 == PortId::new("events_in")
                && edge.role == EdgeRole::Events
        })
        .ok_or_else(|| NativeConventionalError::Missing("source event edge".into()))?;
    used.insert(event.id);
    if let Some(singer) = singer {
        let sung_event = resolved
            .pipeline()
            .edges
            .iter()
            .find(|edge| {
                edge.from.0 == source
                    && edge.from.1 == PortId::new("events_out")
                    && edge.to.0 == singer
                    && edge.to.1 == PortId::new("events_in")
                    && edge.role == EdgeRole::Events
            })
            .ok_or_else(|| NativeConventionalError::Missing("singer event edge".into()))?;
        used.insert(sung_event.id);
    }
    let audio_source = singer.unwrap_or(destination);
    let audio = resolved
        .pipeline()
        .edges
        .iter()
        .find(|edge| {
            edge.from.0 == audio_source
                && edge.from.1 == PortId::new("audio_out")
                && edge.to.0 == input
                && edge.to.1 == PortId::new("audio_in")
                && edge.role == EdgeRole::Main
        })
        .ok_or_else(|| NativeConventionalError::Missing("performer audio edge".into()))?;
    used.insert(audio.id);
    if let Some(singer) = singer {
        let guide = primary.ok_or(NativeConventionalError::Unsupported("singer has no guide"))?;
        let guide_audio = resolved
            .pipeline()
            .edges
            .iter()
            .find(|edge| {
                edge.from.0 == guide
                    && edge.from.1 == PortId::new("audio_out")
                    && edge.to.0 == singer
                    && edge.to.1 == PortId::new("guide_in")
                    && edge.role == EdgeRole::Main
            })
            .ok_or_else(|| NativeConventionalError::Missing("singer guide edge".into()))?;
        used.insert(guide_audio.id);
    }
    Ok(())
}

fn consume_master_output_edges(
    content: &PipelineContent,
    resolved: &auris_core::pipeline::content::ResolvedContent<'_>,
    master_strip: NodeId,
    used: &mut BTreeSet<auris_core::pipeline::EdgeId>,
) -> Result<(), NativeConventionalError> {
    let mut outputs = 0usize;
    for edge in resolved.pipeline().edges.iter().filter(|edge| {
        edge.from.0 == master_strip
            && edge.from.1 == PortId::new("audio_out")
            && edge.to.1 == PortId::new("audio_in")
            && edge.role == EdgeRole::Main
    }) {
        if content.processors.iter().any(|processor| {
            processor.id == edge.to.0 && matches!(&processor.kind, ProcessorKind::Output)
        }) {
            used.insert(edge.id);
            outputs += 1;
        }
    }
    if outputs != 1 {
        return Err(NativeConventionalError::Unsupported(
            "conventional rendering requires one connected master output",
        ));
    }
    Ok(())
}

fn processor<'a>(
    processors: &BTreeMap<NodeId, &'a Processor>,
    id: NodeId,
) -> Result<&'a Processor, NativeConventionalError> {
    processors
        .get(&id)
        .copied()
        .ok_or_else(|| NativeConventionalError::Missing(format!("processor {id:?}")))
}

fn find_group_input(
    content: &PipelineContent,
    group: auris_core::pipeline::content::GroupId,
) -> Result<&Processor, NativeConventionalError> {
    content
        .processors
        .iter()
        .find(|processor| {
            processor.group == Some(group) && matches!(&processor.kind, ProcessorKind::Sum)
        })
        .ok_or_else(|| NativeConventionalError::Missing(format!("sum for group {group:?}")))
}

fn ordered_effects(
    content: &PipelineContent,
    resolved: &auris_core::pipeline::content::ResolvedContent<'_>,
    input: NodeId,
    strip: NodeId,
    slots: &BTreeMap<auris_core::EffectSlotId, NodeId>,
    used: &mut BTreeSet<auris_core::pipeline::EdgeId>,
) -> Result<Vec<EffectSlot>, NativeConventionalError> {
    let mut effects = Vec::new();
    let mut current = input;
    loop {
        let next = resolved.pipeline().edges.iter().find(|edge| {
            edge.from.0 == current
                && edge.from.1 == PortId::new("audio_out")
                && edge.role == EdgeRole::Main
                && edge.to.1 == PortId::new("audio_in")
        });
        let Some(next) = next else {
            return Err(NativeConventionalError::Missing("effect-chain edge".into()));
        };
        let destination = next.to.0;
        if destination == strip {
            used.insert(next.id);
            break;
        }
        let processor = content
            .processors
            .iter()
            .find(|processor| processor.id == destination)
            .ok_or_else(|| NativeConventionalError::Missing(format!("effect {destination:?}")))?;
        let ProcessorKind::Effect {
            slot,
            plugin,
            enabled,
        } = &processor.kind
        else {
            return Err(NativeConventionalError::Unsupported(
                "non-effect in insert chain",
            ));
        };
        let strip_group = content
            .processors
            .iter()
            .find(|node| node.id == strip)
            .and_then(|node| node.group);
        if processor.group != strip_group {
            return Err(NativeConventionalError::Unsupported(
                "insert effects must belong to their conventional group",
            ));
        }
        if slots.get(slot) != Some(&destination) {
            return Err(NativeConventionalError::Unsupported(
                "ambiguous effect slot",
            ));
        }
        used.insert(next.id);
        effects.push(EffectSlot {
            id: *slot,
            effect_id: plugin.plugin_id.clone(),
            enabled: *enabled,
            state: plugin.state.clone(),
            file: plugin.file.clone(),
            sidechain: None,
        });
        current = destination;
    }
    Ok(effects)
}

fn ordered_sends(
    content: &PipelineContent,
    resolved: &auris_core::pipeline::content::ResolvedContent<'_>,
    strip: NodeId,
    sends: &BTreeMap<auris_core::SendId, NodeId>,
    used: &mut BTreeSet<auris_core::pipeline::EdgeId>,
) -> Result<Vec<AuxSend>, NativeConventionalError> {
    let mut output = Vec::new();
    for processor in content.processors.iter().filter(|processor| {
        processor.id != strip
            && processor.group
                == content
                    .processors
                    .iter()
                    .find(|candidate| candidate.id == strip)
                    .and_then(|candidate| candidate.group)
            && matches!(&processor.kind, ProcessorKind::Send { .. })
    }) {
        let ProcessorKind::Send { send, level_db } = &processor.kind else {
            continue;
        };
        if sends.get(send) != Some(&processor.id) {
            return Err(NativeConventionalError::Unsupported("ambiguous send"));
        }
        let incoming = resolved
            .pipeline()
            .edges
            .iter()
            .find(|edge| {
                edge.from.0 == strip
                    && (edge.from.1 == PortId::new("pre_fader")
                        || edge.from.1 == PortId::new("audio_out"))
                    && edge.to.0 == processor.id
                    && edge.to.1 == PortId::new("audio_in")
                    && edge.role == EdgeRole::Main
            })
            .ok_or_else(|| NativeConventionalError::Missing("send input edge".into()))?;
        used.insert(incoming.id);
        let outgoing = resolved
            .pipeline()
            .edges
            .iter()
            .find(|edge| {
                edge.from.0 == processor.id
                    && edge.from.1 == PortId::new("audio_out")
                    && edge.to.1 == PortId::new("audio_in")
                    && edge.role == EdgeRole::Main
            })
            .ok_or_else(|| NativeConventionalError::Missing("send output edge".into()))?;
        used.insert(outgoing.id);
        output.push(AuxSend {
            id: *send,
            target: TrackId(0),
            level_db: *level_db,
            pre_fader: incoming.from.1 == PortId::new("pre_fader"),
        });
    }
    Ok(output)
}

fn lower_routes(
    content: &PipelineContent,
    resolved: &auris_core::pipeline::content::ResolvedContent<'_>,
    project: &mut Project,
    inputs: &BTreeMap<TrackId, NodeId>,
    strips: &BTreeMap<TrackId, NodeId>,
    used: &mut BTreeSet<auris_core::pipeline::EdgeId>,
) -> Result<(), NativeConventionalError> {
    let input_to_track: BTreeMap<NodeId, TrackId> =
        inputs.iter().map(|(track, node)| (*node, *track)).collect();
    let master_input = content
        .processors
        .iter()
        .find(|processor| {
            processor.group == Some(content.master) && matches!(&processor.kind, ProcessorKind::Sum)
        })
        .map(|processor| processor.id)
        .ok_or_else(|| NativeConventionalError::Missing("master sum".into()))?;
    let bus_tracks: BTreeSet<TrackId> = project
        .tracks
        .iter()
        .filter(|track| matches!(&track.kind, TrackKind::Bus))
        .map(|track| track.id)
        .collect();
    for track in &mut project.tracks {
        for send in &mut track.sends {
            let send_node = content
                .processors
                .iter()
                .find_map(|processor| match &processor.kind {
                    ProcessorKind::Send { send: id, .. } if *id == send.id => Some(processor.id),
                    _ => None,
                })
                .ok_or_else(|| NativeConventionalError::Missing(format!("send {:?}", send.id)))?;
            let outgoing = resolved
                .pipeline()
                .edges
                .iter()
                .find(|edge| {
                    edge.from.0 == send_node
                        && edge.from.1 == PortId::new("audio_out")
                        && edge.role == EdgeRole::Main
                        && edge.to.1 == PortId::new("audio_in")
                })
                .ok_or_else(|| NativeConventionalError::Missing("send destination edge".into()))?;
            if outgoing.to.0 == master_input {
                return Err(NativeConventionalError::Unsupported(
                    "sends to the master are not representable by legacy AuxSend",
                ));
            }
            let destination = input_to_track.get(&outgoing.to.0).copied().ok_or(
                NativeConventionalError::Unsupported("send targets a non-bus input"),
            )?;
            if !bus_tracks.contains(&destination) {
                return Err(NativeConventionalError::Unsupported(
                    "send targets a non-bus track",
                ));
            }
            send.target = destination;
        }
    }
    for track in &mut project.tracks {
        let strip = *strips
            .get(&track.id)
            .ok_or_else(|| NativeConventionalError::Missing(format!("strip for {:?}", track.id)))?;
        for candidate in resolved.pipeline().edges.iter().filter(|edge| {
            edge.from.0 == strip
                && edge.from.1 == PortId::new("audio_out")
                && edge.role == EdgeRole::Main
                && edge.to.1 == PortId::new("audio_in")
        }) {
            if candidate.to.0 == master_input {
                track.outputs.push(Output::Master);
            } else if let Some(target) = input_to_track.get(&candidate.to.0).copied() {
                if !bus_tracks.contains(&target) {
                    return Err(NativeConventionalError::Unsupported(
                        "output targets a non-bus track",
                    ));
                }
                track.outputs.push(Output::Bus(target));
            } else {
                continue;
            }
            used.insert(candidate.id);
        }
    }
    for candidate in resolved.pipeline().edges.iter().filter(|edge| {
        edge.role == EdgeRole::Sidechain
            && edge.from.1 == PortId::new("audio_out")
            && edge.to.1 == PortId::new("sidechain_in")
    }) {
        let source = strips
            .iter()
            .find_map(|(track, node)| (*node == candidate.from.0).then_some(*track))
            .ok_or(NativeConventionalError::Unsupported(
                "sidechain source is not a track strip",
            ))?;
        let effect = content
            .processors
            .iter()
            .find(|processor| processor.id == candidate.to.0)
            .ok_or_else(|| NativeConventionalError::Missing("sidechain effect".into()))?;
        let ProcessorKind::Effect { slot, .. } = &effect.kind else {
            return Err(NativeConventionalError::Unsupported(
                "sidechain target is not an effect",
            ));
        };
        let mut found = false;
        for track in &mut project.tracks {
            if let Some(effect) = track
                .mixer
                .effects
                .iter_mut()
                .find(|effect| effect.id == *slot)
            {
                effect.sidechain = Some(source);
                found = true;
            }
        }
        if let Some(effect) = project
            .master
            .effects
            .iter_mut()
            .find(|effect| effect.id == *slot)
        {
            effect.sidechain = Some(source);
            found = true;
        }
        if !found {
            return Err(NativeConventionalError::Missing(format!(
                "effect slot {slot:?}"
            )));
        }
        used.insert(candidate.id);
    }
    Ok(())
}

struct AutomationTargets<'a> {
    strips: &'a BTreeMap<TrackId, NodeId>,
    master_strip: NodeId,
    effect_slots: &'a BTreeMap<auris_core::EffectSlotId, NodeId>,
    instruments: &'a BTreeMap<TrackId, NodeId>,
}

fn lower_automation<P: CapabilityProvider>(
    content: &PipelineContent,
    tracks: &[Track],
    provider: &P,
    processors: &BTreeMap<NodeId, &Processor>,
    targets: AutomationTargets<'_>,
    automation: &mut Automation,
) -> Result<(), NativeConventionalError> {
    let AutomationTargets {
        strips,
        master_strip,
        effect_slots,
        instruments,
    } = targets;
    let strip_tracks: BTreeMap<NodeId, TrackId> =
        strips.iter().map(|(track, node)| (*node, *track)).collect();
    let instrument_tracks: BTreeMap<NodeId, TrackId> = instruments
        .iter()
        .map(|(track, node)| (*node, *track))
        .collect();
    let effect_tracks: BTreeMap<NodeId, Option<TrackId>> = effect_slots
        .iter()
        .map(|(slot, node)| {
            (
                *node,
                tracks
                    .iter()
                    .find(|track| track.mixer.effects.iter().any(|effect| effect.id == *slot))
                    .map(|track| track.id),
            )
        })
        .collect();
    let send_tracks: BTreeMap<_, _> = tracks
        .iter()
        .flat_map(|track| track.sends.iter().map(move |send| (send.id, track.id)))
        .collect();
    for binding in content
        .automation_bindings
        .iter()
        .filter(|binding| binding.enabled)
    {
        let processor = processor(processors, binding.target.node)?;
        let target = match &processor.kind {
            ProcessorKind::Strip(_) => {
                match (strip_tracks.get(&processor.id), binding.target.key.as_str()) {
                    (Some(track), "gain") => ParamTarget::TrackGain(*track),
                    (Some(track), "pan") => ParamTarget::TrackPan(*track),
                    (None, "gain") if processor.id == master_strip => ParamTarget::MasterGain,
                    (None, "pan") if processor.id == master_strip => ParamTarget::MasterPan,
                    _ => {
                        return Err(NativeConventionalError::ParameterKey {
                            node: processor.id,
                            key: binding.target.key.clone(),
                        });
                    }
                }
            }
            ProcessorKind::Send { send, .. } if binding.target.key == "send" => ParamTarget::Send {
                track: *send_tracks
                    .get(send)
                    .ok_or_else(|| NativeConventionalError::Missing("send track".into()))?,
                send: *send,
            },
            ProcessorKind::Instrument(_) => {
                let descriptor = provider
                    .plugin(processor)
                    .and_then(|capabilities| {
                        capabilities
                            .parameters
                            .iter()
                            .find(|parameter| parameter.descriptor.key == binding.target.key)
                    })
                    .ok_or_else(|| NativeConventionalError::ParameterKey {
                        node: processor.id,
                        key: binding.target.key.clone(),
                    })?;
                ParamTarget::Instrument {
                    track: *instrument_tracks.get(&processor.id).ok_or_else(|| {
                        NativeConventionalError::Missing("instrument track".into())
                    })?,
                    param: descriptor.descriptor.id,
                }
            }
            ProcessorKind::Effect { slot, .. } => {
                let descriptor = provider
                    .plugin(processor)
                    .and_then(|capabilities| {
                        capabilities
                            .parameters
                            .iter()
                            .find(|parameter| parameter.descriptor.key == binding.target.key)
                    })
                    .ok_or_else(|| NativeConventionalError::ParameterKey {
                        node: processor.id,
                        key: binding.target.key.clone(),
                    })?;
                ParamTarget::Effect {
                    track: *effect_tracks
                        .get(&processor.id)
                        .ok_or_else(|| NativeConventionalError::Missing("effect track".into()))?,
                    slot: *slot,
                    param: descriptor.descriptor.id,
                }
            }
            _ => {
                return Err(NativeConventionalError::Unsupported(
                    "automation target is not a conventional parameter",
                ));
            }
        };
        let curve_id = content
            .processors
            .iter()
            .find_map(|source| match &source.kind {
                ProcessorKind::Automation(id) if source.id == binding.source => Some(*id),
                _ => None,
            })
            .ok_or_else(|| NativeConventionalError::Missing("automation source".into()))?;
        let curve = content
            .automation
            .iter()
            .find(|curve| curve.id == curve_id)
            .ok_or_else(|| NativeConventionalError::Missing("automation curve".into()))?;
        for point in curve.points() {
            automation.set_point(
                target,
                (!target.is_builtin()).then(|| binding.target.key.clone()),
                curve.curve,
                point.tick,
                point.value,
            );
        }
    }
    Ok(())
}

fn mixer_from_state(state: &auris_core::pipeline::content::StripState) -> MixerStrip {
    MixerStrip {
        gain_db: state.gain_db,
        pan: state.pan,
        mute: state.mute,
        solo: state.solo,
        effects: Vec::new(),
        target_lufs: state.target_lufs,
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use auris_core::pipeline::PipelineLimits;
    use auris_core::pipeline::content::ContentLimits;

    struct NoPlugins;
    impl CapabilityProvider for NoPlugins {
        fn plugin<'a>(
            &'a self,
            _: &Processor,
        ) -> Option<auris_core::pipeline::content::PluginCapabilities<'a>> {
            None
        }
    }

    fn limits() -> (ContentLimits, PipelineLimits) {
        (
            ContentLimits {
                sources: 16,
                clips: 64,
                stages: 64,
                processors: 64,
                groups: 16,
                tracks: 16,
                edges: 128,
                external_ids: 32,
                automations: 16,
                automation_points: 128,
                bindings: 32,
            },
            PipelineLimits {
                nodes: 64,
                edges: 128,
                ports: 256,
                parameter_keys: 128,
                text_bytes: 16 * 1024,
            },
        )
    }

    #[test]
    fn explicit_transform_is_rejected_before_lowering() {
        let project = auris_core::Project::new("native", 48_000.0);
        let mut document = auris_io::project_to_native(&project, limits().0).unwrap();
        document.content.processors.push(Processor {
            id: NodeId(9_000),
            group: Some(document.content.master),
            kind: ProcessorKind::Transform(auris_core::NoteTransform::Lean { ticks: 0 }),
        });
        document.content.next_id = 9_001;
        let (content, graph) = limits();
        assert!(matches!(
            lower_native_conventional(&document, &NoPlugins, content, graph),
            Err(NativeConventionalError::Unsupported(
                "explicit transform processors are reserved for M3"
            ))
        ));
    }

    #[test]
    fn unavailable_plugin_fails_without_mutating_document() {
        let mut project = auris_core::Project::new("native", 48_000.0);
        project.add_instrument_track("Lead", "missing.plugin");
        let (content, graph) = limits();
        let document = auris_io::project_to_native(&project, content).unwrap();
        let before = document.clone();
        assert!(matches!(
            lower_native_conventional(&document, &NoPlugins, content, graph),
            Err(NativeConventionalError::Validation(_))
        ));
        assert_eq!(document, before);
    }
}
