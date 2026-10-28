//! Bounded, lossless checks for the native persistence boundary.
//!
//! This module intentionally does not lower a native graph through the renderer.  A native file
//! can only be projected into the conventional `Project` model when every ownership object and
//! edge has a conventional meaning.  The checks here form that explicit gate and the identity
//! remapper is used when a legacy `Project` is re-encoded after a native migration.

use std::collections::{BTreeMap, BTreeSet};

use auris_core::automation::Automation;
use auris_core::param::{ParamId, ParamTarget};
use auris_core::pipeline::content::{
    AutomationId, ContentLimits, GroupId, PipelineContent, Processor, ProcessorKind, SourceContent,
    SourceDocumentId,
};
use auris_core::pipeline::document::NativeDocument;
use auris_core::pipeline::{EdgeId, EdgeRole, NodeId, PortId};
use auris_core::project::{
    AudioTrack, AuxSend, EffectSlot, InstrumentTrack, MixerStrip, Output, Project, SingerTrack,
    Track, TrackId, TrackKind,
};
use thiserror::Error;

/// Failure proving that a native document is not a lossless conventional projection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum NativePersistenceError {
    /// Native ownership validation failed.
    #[error("native document validation failed: {0}")]
    Validation(String),
    /// A source is shared by multiple conventional tracks.
    #[error("source {0} is shared by multiple tracks")]
    SharedSource(u64),
    /// Disabled control bindings cannot be represented by legacy automation.
    #[error("disabled automation binding {0} is not representable in the conventional model")]
    DisabledAutomation(u64),
    /// One native curve has more than one conventional owner.
    #[error("automation curve {0} is shared by multiple bindings")]
    SharedAutomation(u64),
    /// A native processor kind has no legacy Project counterpart.
    #[error("processor {0} has no lossless conventional representation")]
    UnsupportedProcessor(u64),
    /// A supplied identity mapping is incomplete.
    #[error("native identity mapping is incomplete: {0}")]
    Identity(String),
    /// The native topology is valid but cannot be projected into the legacy model.
    #[error("native document cannot be projected into Project without losing {0}")]
    ProjectionUnsupported(&'static str),
    /// Re-encoding after projection changed persisted native data.
    #[error("native persistence re-encode changed document: {0}")]
    ReencodeMismatch(String),
}

/// Verifies that a native document can be projected without dropping native data.
///
/// This gate is intentionally stricter than renderer preparation: shared sources, disabled or
/// shared automation bindings, transforms, and unknown processor roles are rejected.  Runtime
/// plugin descriptors are not consulted, so opening a file does not require a plugin registry.
pub fn validate_lossless_conventional(
    document: &NativeDocument,
    limits: ContentLimits,
) -> Result<(), NativePersistenceError> {
    document
        .validate_ownership(limits)
        .map_err(|error| NativePersistenceError::Validation(error.to_string()))?;

    let mut source_owners = BTreeMap::<u64, usize>::new();
    for binding in &document.content.tracks {
        if let Some(source) = binding.source {
            let count = source_owners.entry(source.0).or_default();
            *count += 1;
            if *count > 1 {
                return Err(NativePersistenceError::SharedSource(source.0));
            }
        }
    }

    let mut curve_owners = BTreeMap::<u64, usize>::new();
    for binding in &document.content.automation_bindings {
        if !binding.enabled {
            return Err(NativePersistenceError::DisabledAutomation(binding.id.0));
        }
        let count = curve_owners.entry(binding.source.0).or_default();
        *count += 1;
        if *count > 1 {
            return Err(NativePersistenceError::SharedAutomation(binding.source.0));
        }
    }

    let source_ids: BTreeSet<_> = document
        .content
        .sources
        .iter()
        .map(|source| source.id)
        .collect();
    let source_processors: BTreeSet<_> = document
        .content
        .processors
        .iter()
        .filter_map(|processor| match &processor.kind {
            ProcessorKind::Source(source) => Some(*source),
            _ => None,
        })
        .collect();
    if source_ids != source_processors {
        return Err(NativePersistenceError::ProjectionUnsupported(
            "source processor set is not one-to-one",
        ));
    }

    let curve_ids: BTreeSet<_> = document
        .content
        .automation
        .iter()
        .map(|curve| curve.id)
        .collect();
    let curve_processors: BTreeSet<_> = document
        .content
        .processors
        .iter()
        .filter_map(|processor| match &processor.kind {
            ProcessorKind::Automation(curve) => Some(*curve),
            _ => None,
        })
        .collect();
    if curve_ids != curve_processors {
        return Err(NativePersistenceError::ProjectionUnsupported(
            "automation curve processor set is not one-to-one",
        ));
    }
    let bound_curve_ids: BTreeSet<_> = document
        .content
        .automation_bindings
        .iter()
        .filter_map(|binding| {
            document.content.processors.iter().find_map(|processor| {
                (processor.id == binding.source).then_some(match &processor.kind {
                    ProcessorKind::Automation(curve) => Some(*curve),
                    _ => None,
                })?
            })
        })
        .collect();
    if bound_curve_ids != curve_ids {
        return Err(NativePersistenceError::ProjectionUnsupported(
            "unbound automation curve",
        ));
    }

    for processor in &document.content.processors {
        if matches!(&processor.kind, ProcessorKind::Transform(_)) {
            return Err(NativePersistenceError::UnsupportedProcessor(processor.id.0));
        }
    }

    Ok(())
}

/// Converts a legacy project to a native document at the persistence boundary.
///
/// Construction uses the existing bounded converter; native identities are then recovered
/// by semantic owner and connection endpoints, using the immutable checkpoint on Project.
/// No identity is inferred from processor vector order.
pub fn project_to_native_preserving(
    project: &auris_core::Project,
    limits: ContentLimits,
) -> Result<NativeDocument, NativePersistenceError> {
    let mut document = crate::project_to_native(project, limits)
        .map_err(|error| NativePersistenceError::Validation(error.to_string()))?;
    let Some(identity) = project.native_identity() else {
        return Ok(document);
    };
    document.context.source_format_version = identity.context.source_format_version;
    preserve_native_identities(&mut document, identity, limits)?;
    document.content.next_id = document.content.next_id.max(
        project
            .native_allocation_start()
            .ok_or_else(|| NativePersistenceError::Identity("native ID space exhausted".into()))?,
    );
    validate_lossless_conventional(&document, limits)?;
    Ok(document)
}

fn preserve_native_identities(
    document: &mut NativeDocument,
    basis: &NativeDocument,
    limits: ContentLimits,
) -> Result<(), NativePersistenceError> {
    let mut groups = BTreeMap::<GroupId, GroupId>::new();
    let mut sources = BTreeMap::<SourceDocumentId, SourceDocumentId>::new();
    for binding in &document.content.tracks {
        let Some(old) = basis
            .content
            .tracks
            .iter()
            .find(|candidate| candidate.track == binding.track)
        else {
            continue;
        };
        groups.insert(binding.group, old.group);
        if let Some(source) = binding.source
            && let Some(old_source) = old.source
        {
            sources.insert(source, old_source);
        }
    }
    groups.insert(document.content.master, basis.content.master);
    for binding in &mut document.content.tracks {
        binding.group = groups.get(&binding.group).copied().unwrap_or(binding.group);
        binding.source = binding
            .source
            .map(|id| sources.get(&id).copied().unwrap_or(id));
    }
    for group in &mut document.content.groups {
        group.id = groups.get(&group.id).copied().unwrap_or(group.id);
    }
    document.content.master = basis.content.master;
    for source in &mut document.content.sources {
        source.id = sources.get(&source.id).copied().unwrap_or(source.id);
    }

    let mut nodes = BTreeMap::<NodeId, NodeId>::new();
    for generated in &document.content.processors {
        if matches!(generated.kind, ProcessorKind::Automation(_)) {
            continue;
        }
        let old_group = generated
            .group
            .map(|id| groups.get(&id).copied().unwrap_or(id));
        let old = basis.content.processors.iter().find(|candidate| {
            candidate.group == old_group
                && match (&generated.kind, &candidate.kind) {
                    (ProcessorKind::Source(source), ProcessorKind::Source(other)) => {
                        sources.get(source).is_some_and(|mapped| *mapped == *other)
                    }
                    (ProcessorKind::Strip(_), ProcessorKind::Strip(_)) => old_group
                        .and_then(|id| basis.content.groups.iter().find(|g| g.id == id))
                        .is_some_and(|g| g.strip == candidate.id),
                    (ProcessorKind::Sum, ProcessorKind::Sum)
                    | (ProcessorKind::Output, ProcessorKind::Output) => true,
                    (ProcessorKind::Instrument(_), ProcessorKind::Instrument(_)) => old_group
                        .and_then(|id| basis.content.groups.iter().find(|g| g.id == id))
                        .is_some_and(|g| g.primary_instrument == Some(candidate.id)),
                    (ProcessorKind::Singer(_), ProcessorKind::Singer(_)) => true,
                    (
                        ProcessorKind::Effect { slot, .. },
                        ProcessorKind::Effect { slot: other, .. },
                    ) => slot == other,
                    (ProcessorKind::Send { send, .. }, ProcessorKind::Send { send: other, .. }) => {
                        send == other
                    }
                    _ => false,
                }
        });
        nodes.insert(
            generated.id,
            old.map_or(generated.id, |processor| processor.id),
        );
    }
    for processor in &mut document.content.processors {
        if let Some(id) = nodes.get(&processor.id).copied() {
            processor.id = id;
        }
        processor.group = processor
            .group
            .map(|id| groups.get(&id).copied().unwrap_or(id));
        if let ProcessorKind::Source(source) = &mut processor.kind {
            *source = sources.get(&*source).copied().unwrap_or(*source);
        }
    }
    for group in &mut document.content.groups {
        group.strip = nodes[&group.strip];
        group.primary_instrument = group.primary_instrument.map(|id| nodes[&id]);
    }

    let mut automation_ids = BTreeMap::<AutomationId, AutomationId>::new();
    let mut automation_nodes = BTreeMap::<NodeId, NodeId>::new();
    let mut binding_ids = BTreeMap::<EdgeId, EdgeId>::new();
    for generated in &document.content.automation_bindings {
        let target = (nodes[&generated.target.node], generated.target.key.clone());
        let old = basis.content.automation_bindings.iter().find(|candidate| {
            candidate.target.node == target.0 && candidate.target.key == target.1
        });
        let generated_curve = document
            .content
            .processors
            .iter()
            .find(|processor| processor.id == generated.source)
            .and_then(|processor| match &processor.kind {
                ProcessorKind::Automation(id) => Some(*id),
                _ => None,
            })
            .ok_or_else(|| NativePersistenceError::Identity("automation source".into()))?;
        let old_curve = old
            .and_then(|binding| {
                basis
                    .content
                    .processors
                    .iter()
                    .find(|processor| processor.id == binding.source)
                    .and_then(|processor| match &processor.kind {
                        ProcessorKind::Automation(id) => Some(*id),
                        _ => None,
                    })
            })
            .unwrap_or(generated_curve);
        automation_ids.insert(generated_curve, old_curve);
        let old_source = old.map_or(generated.source, |binding| binding.source);
        automation_nodes.insert(generated.source, old_source);
        nodes.insert(generated.source, old_source);
        binding_ids.insert(generated.id, old.map_or(generated.id, |binding| binding.id));
    }
    for curve in &mut document.content.automation {
        curve.id = automation_ids[&curve.id];
    }
    for processor in &mut document.content.processors {
        if let ProcessorKind::Automation(id) = &mut processor.kind {
            *id = automation_ids[&*id];
            processor.id = nodes[&processor.id];
        }
    }
    for binding in &mut document.content.automation_bindings {
        binding.id = binding_ids[&binding.id];
        binding.source = automation_nodes[&binding.source];
        binding.target.node = nodes[&binding.target.node];
    }
    if nodes.len() != document.content.processors.len() {
        return Err(NativePersistenceError::Identity(
            "processor role mapping is incomplete".into(),
        ));
    }

    let mut used_edges = BTreeSet::new();
    for edge in &mut document.content.edges {
        let from = nodes[&edge.from.0];
        let to = nodes[&edge.to.0];
        edge.from.0 = from;
        edge.to.0 = to;
        let old = basis.content.edges.iter().find(|candidate| {
            candidate.from.0 == from
                && candidate.from.1 == edge.from.1
                && candidate.to.0 == to
                && candidate.to.1 == edge.to.1
                && candidate.role == edge.role
                && used_edges.insert(candidate.id)
        });
        if let Some(old) = old {
            edge.id = old.id;
        }
    }
    let actual_next = content_allocation_start(&document.content)
        .ok_or_else(|| NativePersistenceError::Identity("native ID space exhausted".into()))?;
    document.content.next_id = actual_next
        .max(basis.content.next_id)
        .max(project_allocation_start_from_context(document));
    document
        .validate_ownership(limits)
        .map_err(|error| NativePersistenceError::Validation(error.to_string()))?;
    Ok(())
}

fn project_allocation_start_from_context(document: &NativeDocument) -> u64 {
    let maximum = document
        .context
        .audio_sources
        .keys()
        .map(|id| id.0)
        .chain(document.context.soundfonts.keys().map(|id| id.0))
        .max()
        .unwrap_or(0);
    document
        .context
        .source_next_id
        .max(maximum.saturating_add(1))
}

fn content_allocation_start(content: &PipelineContent) -> Option<u64> {
    let mut maximum = 0u64;
    for id in content
        .processors
        .iter()
        .map(|item| item.id.0)
        .chain(content.groups.iter().map(|item| item.id.0))
        .chain(content.sources.iter().map(|item| item.id.0))
        .chain(content.automation.iter().map(|item| item.id.0))
        .chain(content.tracks.iter().map(|item| item.track.0))
        .chain(content.edges.iter().map(|item| item.id.0))
        .chain(content.automation_bindings.iter().map(|item| item.id.0))
    {
        maximum = maximum.max(id);
    }
    for processor in &content.processors {
        match &processor.kind {
            ProcessorKind::Effect { slot, .. } => maximum = maximum.max(slot.0),
            ProcessorKind::Send { send, .. } => maximum = maximum.max(send.0),
            _ => {}
        }
    }
    for source in &content.sources {
        match &source.content {
            SourceContent::Notes(clips) => {
                for clip in clips {
                    maximum = maximum.max(clip.id.0);
                    for stage in &clip.transforms {
                        maximum = maximum.max(stage.id.0);
                    }
                }
            }
            SourceContent::Audio(clips) => {
                for clip in clips {
                    maximum = maximum.max(clip.id.0);
                }
            }
        }
    }
    maximum.checked_add(1)
}

/// Projects a native document into the conventional model only through the explicit lossless
/// gate.
///
/// Unlike renderer lowering, persistence decoding does not need installed plugin descriptors.
/// Every authored field and identity must survive re-encoding before the projection is accepted.
pub fn native_to_project(
    document: &NativeDocument,
    limits: ContentLimits,
) -> Result<Project, NativePersistenceError> {
    validate_lossless_conventional(document, limits)?;
    let mut project = decode_conventional(document)?;
    project.set_native_identity(std::sync::Arc::new(document.clone()));
    let mut encoded = project_to_native_preserving(&project, limits)?;
    let mut original = document.clone();
    canonicalize_collections(&mut encoded);
    canonicalize_collections(&mut original);
    if encoded != original {
        return Err(NativePersistenceError::ReencodeMismatch(
            "context, topology, payload, or stable identity differs".into(),
        ));
    }
    Ok(project)
}

fn canonicalize_collections(document: &mut NativeDocument) {
    document.content.sources.sort_by_key(|entry| entry.id);
    document.content.groups.sort_by_key(|entry| entry.id);
    document.content.processors.sort_by_key(|entry| entry.id);
    document.content.automation.sort_by_key(|entry| entry.id);
    document
        .content
        .automation_bindings
        .sort_by_key(|entry| entry.id);
    document.content.edges.sort_by_key(|entry| entry.id);
    document.content.tracks.sort_by_key(|entry| entry.track);
}

/// Decodes and re-encodes a native document, accepting it only when the persistence projection
/// is lossless.  The encoder is expected to reuse the immutable identity checkpoint; therefore a
/// byte-for-byte comparison is intentionally not used, while the typed document equality is.
pub fn native_to_project_lossless(
    document: &NativeDocument,
    limits: ContentLimits,
) -> Result<Project, NativePersistenceError> {
    native_to_project(document, limits)
}

fn decode_conventional(document: &NativeDocument) -> Result<Project, NativePersistenceError> {
    let content = &document.content;
    let processors: BTreeMap<NodeId, &Processor> =
        content.processors.iter().map(|p| (p.id, p)).collect();
    let processor = |id: NodeId| {
        processors.get(&id).copied().ok_or_else(|| {
            NativePersistenceError::Validation(format!("missing processor {}", id.0))
        })
    };
    let mut inputs = BTreeMap::new();
    let mut strips = BTreeMap::new();
    let mut instrument_nodes = BTreeMap::new();
    let mut singer_nodes = BTreeMap::new();
    let mut used = BTreeSet::new();
    let mut tracks = Vec::with_capacity(document.track_presentation.len());

    for presentation in &document.track_presentation {
        let binding = content
            .tracks
            .iter()
            .find(|b| b.track == presentation.id)
            .ok_or_else(|| {
                NativePersistenceError::Validation(format!("missing track {}", presentation.id.0))
            })?;
        let group = content
            .groups
            .iter()
            .find(|g| g.id == binding.group)
            .ok_or_else(|| NativePersistenceError::Validation("missing group".into()))?;
        let strip = processor(group.strip)?;
        let ProcessorKind::Strip(strip_state) = &strip.kind else {
            return Err(NativePersistenceError::ProjectionUnsupported("group strip"));
        };
        let input = content
            .processors
            .iter()
            .find(|p| p.group == Some(group.id) && matches!(&p.kind, ProcessorKind::Sum))
            .ok_or_else(|| NativePersistenceError::Validation("missing group sum".into()))?;
        inputs.insert(presentation.id, input.id);
        strips.insert(presentation.id, strip.id);
        let source = match binding.source {
            Some(id) => content
                .sources
                .iter()
                .find(|s| s.id == id)
                .ok_or_else(|| {
                    NativePersistenceError::Validation(format!(
                        "missing source for track {}",
                        presentation.id.0
                    ))
                })
                .map(Some),
            None if presentation.kind == auris_core::pipeline::document::NativeTrackKind::Bus => {
                Ok(None)
            }
            None => Err(NativePersistenceError::Validation(format!(
                "missing source for track {}",
                presentation.id.0
            ))),
        }?;
        let primary = group.primary_instrument.map(processor).transpose()?;
        if let Some(primary) = primary {
            instrument_nodes.insert(presentation.id, primary.id);
        }
        let singer = content
            .processors
            .iter()
            .find(|p| p.group == Some(group.id) && matches!(&p.kind, ProcessorKind::Singer(_)));
        if let Some(singer) = singer {
            singer_nodes.insert(presentation.id, singer.id);
        }
        let kind = match presentation.kind {
            auris_core::pipeline::document::NativeTrackKind::Bus => TrackKind::Bus,
            auris_core::pipeline::document::NativeTrackKind::Audio => {
                let source = source
                    .ok_or_else(|| NativePersistenceError::Validation("audio source".into()))?;
                let SourceContent::Audio(clips) = &source.content else {
                    return Err(NativePersistenceError::ProjectionUnsupported(
                        "audio source role",
                    ));
                };
                TrackKind::Audio(AudioTrack {
                    clips: clips.clone(),
                })
            }
            auris_core::pipeline::document::NativeTrackKind::Instrument
            | auris_core::pipeline::document::NativeTrackKind::Drum => {
                let source = source
                    .ok_or_else(|| NativePersistenceError::Validation("note source".into()))?;
                let SourceContent::Notes(clips) = &source.content else {
                    return Err(NativePersistenceError::ProjectionUnsupported(
                        "note source role",
                    ));
                };
                let Some(primary) = primary else {
                    return Err(NativePersistenceError::Validation(
                        "missing instrument".into(),
                    ));
                };
                let ProcessorKind::Instrument(instance) = &primary.kind else {
                    return Err(NativePersistenceError::ProjectionUnsupported(
                        "primary instrument role",
                    ));
                };
                let instrument = InstrumentTrack {
                    instrument_id: instance.plugin_id.clone(),
                    instrument_state: instance.state.clone(),
                    clips: clips.iter().map(|c| c.performer_clip()).collect(),
                    file: instance.file.clone(),
                };
                if presentation.kind == auris_core::pipeline::document::NativeTrackKind::Drum {
                    TrackKind::Drum(instrument)
                } else {
                    TrackKind::Instrument(instrument)
                }
            }
            auris_core::pipeline::document::NativeTrackKind::Singer => {
                let source = source
                    .ok_or_else(|| NativePersistenceError::Validation("singer source".into()))?;
                let SourceContent::Notes(clips) = &source.content else {
                    return Err(NativePersistenceError::ProjectionUnsupported(
                        "singer source role",
                    ));
                };
                let Some(primary) = primary else {
                    return Err(NativePersistenceError::Validation(
                        "missing singer guide".into(),
                    ));
                };
                let ProcessorKind::Instrument(guide) = &primary.kind else {
                    return Err(NativePersistenceError::ProjectionUnsupported(
                        "singer guide role",
                    ));
                };
                let Some(singer) = singer else {
                    return Err(NativePersistenceError::Validation(
                        "missing singer processor".into(),
                    ));
                };
                let ProcessorKind::Singer(instance) = &singer.kind else {
                    return Err(NativePersistenceError::ProjectionUnsupported("singer role"));
                };
                TrackKind::Singer(SingerTrack {
                    instrument_id: guide.plugin_id.clone(),
                    instrument_state: guide.state.clone(),
                    clips: clips.iter().map(|c| c.performer_clip()).collect(),
                    frame_hop: instance.frame_hop,
                    voice: instance.voice.clone(),
                    take: instance.take.clone(),
                })
            }
        };
        let effects = decode_effects(content, &processors, input.id, strip.id, &mut used)?;
        let sends = decode_sends(content, &processors, strip.id, &mut used)?;
        tracks.push(Track {
            id: presentation.id,
            name: presentation.name.clone(),
            color: presentation.color,
            height: presentation.height,
            kind,
            mixer: MixerStrip {
                gain_db: strip_state.gain_db,
                pan: strip_state.pan,
                mute: strip_state.mute,
                solo: strip_state.solo,
                effects,
                target_lufs: strip_state.target_lufs,
            },
            outputs: Vec::new(),
            sends,
        });
        if let Some(source) = source {
            consume_event_audio_edges(
                content,
                source,
                primary.map(|p| p.id),
                singer.map(|p| p.id),
                input.id,
                &mut used,
            )?;
        }
    }
    let mut project = Project::new(document.context.name.clone(), document.context.sample_rate);
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
    let master = content
        .groups
        .iter()
        .find(|g| g.id == content.master)
        .ok_or_else(|| NativePersistenceError::Validation("missing master group".into()))?;
    let master_strip = processor(master.strip)?;
    let ProcessorKind::Strip(master_state) = &master_strip.kind else {
        return Err(NativePersistenceError::ProjectionUnsupported(
            "master strip",
        ));
    };
    let master_input = content
        .processors
        .iter()
        .find(|p| p.group == Some(content.master) && matches!(&p.kind, ProcessorKind::Sum))
        .ok_or_else(|| NativePersistenceError::Validation("missing master sum".into()))?;
    project.master = MixerStrip {
        gain_db: master_state.gain_db,
        pan: master_state.pan,
        mute: master_state.mute,
        solo: master_state.solo,
        effects: decode_effects(
            content,
            &processors,
            master_input.id,
            master_strip.id,
            &mut used,
        )?,
        target_lufs: master_state.target_lufs,
    };
    for edge in content.edges.iter().filter(|edge| {
        edge.from.0 == master_strip.id
            && edge.from.1 == PortId::new("audio_out")
            && edge.to.1 == PortId::new("audio_in")
            && edge.role == EdgeRole::Main
            && content.processors.iter().any(|processor| {
                processor.id == edge.to.0 && matches!(&processor.kind, ProcessorKind::Output)
            })
    }) {
        used.insert(edge.id);
    }
    for track in &mut tracks {
        decode_outputs(
            content,
            track,
            strips[&track.id],
            master_input.id,
            &inputs,
            &mut used,
        )?;
        decode_send_targets(content, track, &inputs, &mut used)?;
    }
    project.tracks = tracks;
    for edge in content
        .edges
        .iter()
        .filter(|edge| edge.role == EdgeRole::Sidechain)
    {
        let source = strips
            .iter()
            .find_map(|(track, node)| (*node == edge.from.0).then_some(*track))
            .filter(|_| {
                edge.from.1 == PortId::new("audio_out") && edge.to.1 == PortId::new("sidechain_in")
            })
            .ok_or(NativePersistenceError::ProjectionUnsupported(
                "sidechain source",
            ))?;
        let ProcessorKind::Effect { slot, .. } = &processor(edge.to.0)?.kind else {
            return Err(NativePersistenceError::ProjectionUnsupported(
                "sidechain target",
            ));
        };
        let effect = project
            .master
            .effects
            .iter_mut()
            .chain(
                project
                    .tracks
                    .iter_mut()
                    .flat_map(|track| &mut track.mixer.effects),
            )
            .find(|effect| effect.id == *slot)
            .ok_or(NativePersistenceError::ProjectionUnsupported(
                "sidechain effect",
            ))?;
        if effect.sidechain.replace(source).is_some() {
            return Err(NativePersistenceError::ProjectionUnsupported(
                "multiple sidechains",
            ));
        }
        used.insert(edge.id);
    }
    decode_automation(
        content,
        &mut project.automation,
        &project.tracks,
        &instrument_nodes,
        &strips,
        master_strip.id,
    )?;
    if used.len() != content.edges.len() {
        return Err(NativePersistenceError::ProjectionUnsupported(
            "unconsumed native edge",
        ));
    }
    if !project.restore_allocation_counter(document.context.source_next_id)
        || !project.routing_is_valid()
    {
        return Err(NativePersistenceError::ProjectionUnsupported(
            "legacy routing or IDs",
        ));
    }
    Ok(project)
}

fn edge_between<'a>(
    content: &'a PipelineContent,
    from: NodeId,
    from_port: &str,
    to: NodeId,
    to_port: &str,
    role: EdgeRole,
) -> Option<&'a auris_core::pipeline::PipelineEdge> {
    content.edges.iter().find(|e| {
        e.from == (from, PortId::new(from_port))
            && e.to == (to, PortId::new(to_port))
            && e.role == role
    })
}

fn decode_effects(
    content: &PipelineContent,
    processors: &BTreeMap<NodeId, &Processor>,
    input: NodeId,
    strip: NodeId,
    used: &mut BTreeSet<EdgeId>,
) -> Result<Vec<EffectSlot>, NativePersistenceError> {
    let mut result = Vec::new();
    let mut current = input;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current) {
            return Err(NativePersistenceError::ProjectionUnsupported(
                "cyclic conventional insert chain",
            ));
        }
        let edge = edge_between(
            content,
            current,
            "audio_out",
            strip,
            "audio_in",
            EdgeRole::Main,
        )
        .or_else(|| {
            content.edges.iter().find(|e| {
                e.from.0 == current
                    && e.from.1 == PortId::new("audio_out")
                    && e.to.1 == PortId::new("audio_in")
                    && e.role == EdgeRole::Main
            })
        });
        let Some(edge) = edge else {
            return Err(NativePersistenceError::Validation(
                "broken insert chain".into(),
            ));
        };
        if !used.insert(edge.id) {
            return Err(NativePersistenceError::ProjectionUnsupported(
                "edge is consumed by multiple conventional paths",
            ));
        }
        if edge.to.0 == strip {
            break;
        }
        let node = processors
            .get(&edge.to.0)
            .copied()
            .ok_or_else(|| NativePersistenceError::Validation("missing effect".into()))?;
        let ProcessorKind::Effect {
            slot,
            plugin,
            enabled,
        } = &node.kind
        else {
            return Err(NativePersistenceError::ProjectionUnsupported(
                "non-effect insert",
            ));
        };
        result.push(EffectSlot {
            id: *slot,
            effect_id: plugin.plugin_id.clone(),
            enabled: *enabled,
            state: plugin.state.clone(),
            file: plugin.file.clone(),
            sidechain: None,
        });
        current = node.id;
    }
    Ok(result)
}

fn decode_sends(
    content: &PipelineContent,
    processors: &BTreeMap<NodeId, &Processor>,
    strip: NodeId,
    used: &mut BTreeSet<EdgeId>,
) -> Result<Vec<AuxSend>, NativePersistenceError> {
    let group = processors
        .get(&strip)
        .and_then(|p| p.group)
        .ok_or_else(|| NativePersistenceError::Validation("strip group".into()))?;
    let mut result = Vec::new();
    for node in content
        .processors
        .iter()
        .filter(|p| p.group == Some(group) && matches!(&p.kind, ProcessorKind::Send { .. }))
    {
        let ProcessorKind::Send { send, level_db } = &node.kind else {
            continue;
        };
        let input = content
            .edges
            .iter()
            .find(|e| e.from.0 == strip && e.to.0 == node.id && e.role == EdgeRole::Main)
            .ok_or_else(|| NativePersistenceError::Validation("send input".into()))?;
        used.insert(input.id);
        result.push(AuxSend {
            id: *send,
            target: TrackId(0),
            level_db: *level_db,
            pre_fader: input.from.1 == PortId::new("pre_fader"),
        });
    }
    Ok(result)
}

fn consume_event_audio_edges(
    content: &PipelineContent,
    source: &auris_core::pipeline::content::SourceDocument,
    instrument: Option<NodeId>,
    singer: Option<NodeId>,
    input: NodeId,
    used: &mut BTreeSet<EdgeId>,
) -> Result<(), NativePersistenceError> {
    let source_node = content
        .processors
        .iter()
        .find(|p| matches!(&p.kind, ProcessorKind::Source(id) if *id == source.id))
        .ok_or_else(|| NativePersistenceError::Validation("source node".into()))?;
    if let Some(instrument) = instrument
        && let Some(e) = content.edges.iter().find(|e| {
            e.from.0 == source_node.id && e.to.0 == instrument && e.role == EdgeRole::Events
        })
    {
        used.insert(e.id);
    }
    if let Some(singer) = singer {
        if let Some(e) = content
            .edges
            .iter()
            .find(|e| e.from.0 == source_node.id && e.to.0 == singer && e.role == EdgeRole::Events)
        {
            used.insert(e.id);
        }
        if let Some(instrument) = instrument {
            let guide = edge_between(
                content,
                instrument,
                "audio_out",
                singer,
                "guide_in",
                EdgeRole::Main,
            )
            .ok_or_else(|| NativePersistenceError::Validation("singer guide edge".into()))?;
            used.insert(guide.id);
        }
    }
    let e = match &source.content {
        SourceContent::Audio(_) => content
            .edges
            .iter()
            .find(|e| e.from.0 == source_node.id && e.to.0 == input && e.role == EdgeRole::Main)
            .ok_or_else(|| NativePersistenceError::Validation("audio source edge".into()))?,
        SourceContent::Notes(_) => {
            let audio = singer
                .or(instrument)
                .ok_or_else(|| NativePersistenceError::Validation("performer".into()))?;
            content
                .edges
                .iter()
                .find(|e| e.from.0 == audio && e.to.0 == input && e.role == EdgeRole::Main)
                .ok_or_else(|| NativePersistenceError::Validation("performer audio".into()))?
        }
    };
    used.insert(e.id);
    Ok(())
}

fn decode_outputs(
    content: &PipelineContent,
    track: &mut Track,
    strip: NodeId,
    master_input: NodeId,
    inputs: &BTreeMap<TrackId, NodeId>,
    used: &mut BTreeSet<EdgeId>,
) -> Result<(), NativePersistenceError> {
    for edge in content.edges.iter().filter(|e| {
        e.from.0 == strip && e.role == EdgeRole::Main && e.to.1 == PortId::new("audio_in")
    }) {
        if used.contains(&edge.id) {
            continue;
        }
        if edge.to.0 == master_input {
            track.outputs.push(Output::Master);
        } else if let Some(target) = inputs
            .iter()
            .find_map(|(id, node)| (*node == edge.to.0).then_some(*id))
        {
            track.outputs.push(Output::Bus(target));
        } else {
            return Err(NativePersistenceError::ProjectionUnsupported(
                "output target",
            ));
        }
        used.insert(edge.id);
    }
    Ok(())
}

fn decode_send_targets(
    content: &PipelineContent,
    track: &mut Track,
    inputs: &BTreeMap<TrackId, NodeId>,
    used: &mut BTreeSet<EdgeId>,
) -> Result<(), NativePersistenceError> {
    for send in &mut track.sends {
        let node = content
            .processors
            .iter()
            .find(|p| matches!(&p.kind, ProcessorKind::Send { send: id, .. } if *id == send.id))
            .ok_or_else(|| NativePersistenceError::Validation("send node".into()))?;
        let edge = content
            .edges
            .iter()
            .find(|e| {
                e.from.0 == node.id && e.role == EdgeRole::Main && e.to.1 == PortId::new("audio_in")
            })
            .ok_or_else(|| NativePersistenceError::Validation("send target".into()))?;
        send.target = inputs
            .iter()
            .find_map(|(id, input)| (*input == edge.to.0).then_some(*id))
            .ok_or(NativePersistenceError::ProjectionUnsupported("send target"))?;
        used.insert(edge.id);
    }
    Ok(())
}

fn decode_automation(
    content: &PipelineContent,
    automation: &mut Automation,
    tracks: &[Track],
    instruments: &BTreeMap<TrackId, NodeId>,
    strips: &BTreeMap<TrackId, NodeId>,
    master_strip: NodeId,
) -> Result<(), NativePersistenceError> {
    let mut keys = BTreeMap::<NodeId, BTreeSet<String>>::new();
    for binding in &content.automation_bindings {
        keys.entry(binding.target.node)
            .or_default()
            .insert(binding.target.key.clone());
    }
    let mut param_ids = BTreeMap::new();
    for (node, set) in keys {
        param_ids.insert(
            node,
            set.into_iter()
                .enumerate()
                .map(|(i, k)| (k, ParamId(i as u32)))
                .collect::<BTreeMap<_, _>>(),
        );
    }
    let track_by_strip: BTreeMap<NodeId, TrackId> =
        strips.iter().map(|(id, node)| (*node, *id)).collect();
    let instrument_by_node: BTreeMap<NodeId, TrackId> =
        instruments.iter().map(|(id, node)| (*node, *id)).collect();
    for binding in content.automation_bindings.iter().filter(|b| b.enabled) {
        let target = match content
            .processors
            .iter()
            .find(|p| p.id == binding.target.node)
            .map(|p| &p.kind)
        {
            Some(ProcessorKind::Strip(_)) => match (
                track_by_strip.get(&binding.target.node),
                binding.target.key.as_str(),
            ) {
                (Some(track), "gain") => ParamTarget::TrackGain(*track),
                (Some(track), "pan") => ParamTarget::TrackPan(*track),
                (None, "gain") if binding.target.node == master_strip => ParamTarget::MasterGain,
                (None, "pan") if binding.target.node == master_strip => ParamTarget::MasterPan,
                _ => {
                    return Err(NativePersistenceError::ProjectionUnsupported(
                        "strip automation key",
                    ));
                }
            },
            Some(ProcessorKind::Send { send, .. }) if binding.target.key == "send" => {
                let track = tracks
                    .iter()
                    .find(|t| t.sends.iter().any(|s| s.id == *send))
                    .ok_or_else(|| {
                        NativePersistenceError::Validation("send automation owner".into())
                    })?;
                ParamTarget::Send {
                    track: track.id,
                    send: *send,
                }
            }
            Some(ProcessorKind::Instrument(_)) => ParamTarget::Instrument {
                track: *instrument_by_node
                    .get(&binding.target.node)
                    .ok_or_else(|| {
                        NativePersistenceError::Validation("instrument automation owner".into())
                    })?,
                param: param_ids[&binding.target.node][&binding.target.key],
            },
            Some(ProcessorKind::Effect { slot, .. }) => ParamTarget::Effect {
                track: tracks
                    .iter()
                    .find(|t| t.mixer.effects.iter().any(|e| e.id == *slot))
                    .map(|t| t.id),
                slot: *slot,
                param: param_ids[&binding.target.node][&binding.target.key],
            },
            _ => {
                return Err(NativePersistenceError::ProjectionUnsupported(
                    "automation target",
                ));
            }
        };
        let source = content
            .processors
            .iter()
            .find(|p| p.id == binding.source)
            .and_then(|p| match &p.kind {
                ProcessorKind::Automation(id) => Some(*id),
                _ => None,
            })
            .ok_or_else(|| NativePersistenceError::Validation("automation source".into()))?;
        let curve = content
            .automation
            .iter()
            .find(|c| c.id == source)
            .ok_or_else(|| NativePersistenceError::Validation("automation curve".into()))?;
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

#[cfg(test)]
mod tests;
