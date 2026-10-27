//! Explicit, pre-activation conversion from the legacy project model.
//!
//! The converter borrows the input and builds a candidate from scratch. It never loads a plugin,
//! mutates the legacy project, or changes the project writer. A failed conversion therefore leaves
//! the caller with the original document and a structured reason to defer activation.

use auris_core::ParamTarget;
use auris_core::pipeline::ControlDomain;
use auris_core::pipeline::content::{
    ContentLimits, PipelineContent, PluginInstance, ProcessingGroup, Processor, ProcessorKind,
    SingerInstance, SourceContent, SourceDocument, SourceDocumentId, StripState, TrackBinding,
};
use auris_core::pipeline::document::{
    NativeDocument, NativeProjectContext, NativeTrackKind, TrackPresentation,
};
use auris_core::pipeline::{EdgeId, NodeId, PipelineEdge, PortId};
use auris_core::project::{Project, TrackKind};
use thiserror::Error;

/// A conversion failure that is safe to report before changing project state.
#[derive(Debug, Error)]
pub enum PipelineConversionError {
    /// The source contains a plugin automation lane without an authored stable key.
    #[error("automation lane {index} addresses a plugin parameter without a stable key")]
    MissingPluginKey {
        /// Position of the lane in the source automation collection.
        index: usize,
    },
    /// Source IDs conflict or an asset key disagrees with its payload identity.
    #[error("legacy object ID {0} is inconsistent")]
    InvalidLegacyId(u64),
    /// The input must have passed the existing loader's explicit routing repair.
    #[error("legacy routing contains invalid references or cycles")]
    InvalidLegacyRouting,
    /// The source ID allocator cannot provide another native identity.
    #[error("native pipeline ID space is exhausted")]
    IdExhausted,
    /// The native ownership candidate rejected the conversion.
    #[error("native ownership validation failed: {0}")]
    Ownership(#[from] auris_core::pipeline::content::ContentError),
    /// A legacy clip stage conversion failed.
    #[error("clip-stage conversion failed: {0}")]
    Stage(#[from] auris_core::project::StageError),
    /// A legacy automation curve cannot be represented by the native curve validator.
    #[error("automation conversion failed: {0}")]
    Automation(#[from] auris_core::pipeline::content::AutomationError),
}

struct Ids {
    next: u64,
}

impl Ids {
    fn take(&mut self) -> Result<u64, PipelineConversionError> {
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(PipelineConversionError::IdExhausted)?;
        Ok(id)
    }
}

/// Converts a decoded legacy project into a standalone native document.
pub fn project_to_native(
    project: &Project,
    limits: ContentLimits,
) -> Result<NativeDocument, PipelineConversionError> {
    let mut ids = Ids {
        next: preflight(project, limits)?,
    };
    let master_group = auris_core::pipeline::content::GroupId(ids.take()?);
    let master_strip = NodeId(ids.take()?);
    let mut content = PipelineContent {
        sources: Vec::new(),
        automation: Vec::new(),
        processors: vec![Processor {
            id: master_strip,
            group: Some(master_group),
            kind: ProcessorKind::Strip(strip_state(&project.master)),
        }],
        groups: vec![ProcessingGroup {
            id: master_group,
            strip: master_strip,
            primary_instrument: None,
        }],
        tracks: Vec::new(),
        edges: Vec::new(),
        automation_bindings: Vec::new(),
        master: master_group,
        next_id: ids.next,
    };
    let master_input = NodeId(ids.take()?);
    content.processors.push(Processor {
        id: master_input,
        group: Some(master_group),
        kind: ProcessorKind::Sum,
    });
    let mut strips = std::collections::BTreeMap::new();
    let mut inputs = std::collections::BTreeMap::new();
    let mut instruments = std::collections::BTreeMap::new();
    let mut singers = std::collections::BTreeMap::new();
    let mut source_nodes = std::collections::BTreeMap::new();
    let mut sends = std::collections::BTreeMap::new();
    let mut effects = std::collections::BTreeMap::new();

    for track in &project.tracks {
        let group = auris_core::pipeline::content::GroupId(ids.take()?);
        let strip = NodeId(ids.take()?);
        let input = NodeId(ids.take()?);
        inputs.insert(track.id, input);
        strips.insert(track.id, strip);
        let primary = match &track.kind {
            TrackKind::Instrument(instrument) | TrackKind::Drum(instrument) => {
                let node = NodeId(ids.take()?);
                instruments.insert(track.id, node);
                content.processors.push(Processor {
                    id: node,
                    group: Some(group),
                    kind: ProcessorKind::Instrument(PluginInstance {
                        plugin_id: instrument.instrument_id.clone(),
                        state: instrument.instrument_state.clone(),
                        file: instrument.file.clone(),
                    }),
                });
                Some(node)
            }
            TrackKind::Singer(singer) => {
                let guide = NodeId(ids.take()?);
                instruments.insert(track.id, guide);
                content.processors.push(Processor {
                    id: guide,
                    group: Some(group),
                    kind: ProcessorKind::Instrument(PluginInstance {
                        plugin_id: singer.instrument_id.clone(),
                        state: singer.instrument_state.clone(),
                        file: None,
                    }),
                });
                let singer_node = NodeId(ids.take()?);
                singers.insert(track.id, singer_node);
                content.processors.push(Processor {
                    id: singer_node,
                    group: Some(group),
                    kind: ProcessorKind::Singer(Box::new(SingerInstance {
                        frame_hop: singer.frame_hop,
                        voice: singer.voice.clone(),
                        take: singer.take.clone(),
                    })),
                });
                Some(guide)
            }
            TrackKind::Audio(_) | TrackKind::Bus => None,
        };
        content.processors.push(Processor {
            id: strip,
            group: Some(group),
            kind: ProcessorKind::Strip(strip_state(&track.mixer)),
        });
        content.processors.push(Processor {
            id: input,
            group: Some(group),
            kind: ProcessorKind::Sum,
        });
        content.groups.push(ProcessingGroup {
            id: group,
            strip,
            primary_instrument: primary,
        });
        let mut binding = TrackBinding {
            track: track.id,
            source: None,
            group,
        };

        if let Some(kind) = track.kind.note_clips() {
            let source_id = SourceDocumentId(ids.take()?);
            let mut clips = Vec::with_capacity(kind.len());
            for clip in kind {
                let stage_ids = clip
                    .transforms
                    .iter()
                    .map(|_| ids.take().map(auris_core::project::StageId))
                    .collect::<Result<Vec<_>, _>>()?;
                clips.push(clip.clone().with_stage_ids(stage_ids)?);
            }
            content.sources.push(SourceDocument {
                id: source_id,
                content: SourceContent::Notes(clips),
            });
            binding.source = Some(source_id);
            let source_node = NodeId(ids.take()?);
            source_nodes.insert(track.id, source_node);
            content.processors.push(Processor {
                id: source_node,
                group: None,
                kind: ProcessorKind::Source(source_id),
            });
        } else if let TrackKind::Audio(audio) = &track.kind {
            let source_id = SourceDocumentId(ids.take()?);
            content.sources.push(SourceDocument {
                id: source_id,
                content: SourceContent::Audio(audio.clips.clone()),
            });
            binding.source = Some(source_id);
            let source_node = NodeId(ids.take()?);
            source_nodes.insert(track.id, source_node);
            content.processors.push(Processor {
                id: source_node,
                group: None,
                kind: ProcessorKind::Source(source_id),
            });
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (source_node, PortId::new("audio_out")),
                to: (input, PortId::new("audio_in")),
                role: auris_core::pipeline::EdgeRole::Main,
            });
        }

        content.tracks.push(binding);
        for effect in &track.mixer.effects {
            let node = NodeId(ids.take()?);
            effects.insert(effect.id, node);
            content.processors.push(Processor {
                id: node,
                group: Some(group),
                kind: ProcessorKind::Effect {
                    slot: effect.id,
                    plugin: PluginInstance {
                        plugin_id: effect.effect_id.clone(),
                        state: effect.state.clone(),
                        file: effect.file.clone(),
                    },
                    enabled: effect.enabled,
                },
            });
        }
        for send in &track.sends {
            let node = NodeId(ids.take()?);
            sends.insert(send.id, node);
            content.processors.push(Processor {
                id: node,
                group: Some(group),
                kind: ProcessorKind::Send {
                    send: send.id,
                    level_db: send.level_db,
                },
            });
        }
    }

    // All incoming routes sum before the first insert, including parallel outputs and sends.
    let mut master_previous = master_input;
    for effect in &project.master.effects {
        let node = NodeId(ids.take()?);
        effects.insert(effect.id, node);
        content.edges.push(PipelineEdge {
            id: EdgeId(ids.take()?),
            from: (master_previous, PortId::new("audio_out")),
            to: (node, PortId::new("audio_in")),
            role: auris_core::pipeline::EdgeRole::Main,
        });
        content.processors.push(Processor {
            id: node,
            group: Some(master_group),
            kind: ProcessorKind::Effect {
                slot: effect.id,
                plugin: PluginInstance {
                    plugin_id: effect.effect_id.clone(),
                    state: effect.state.clone(),
                    file: effect.file.clone(),
                },
                enabled: effect.enabled,
            },
        });
        master_previous = node;
    }
    content.edges.push(PipelineEdge {
        id: EdgeId(ids.take()?),
        from: (master_previous, PortId::new("audio_out")),
        to: (master_strip, PortId::new("audio_in")),
        role: auris_core::pipeline::EdgeRole::Main,
    });
    let output = NodeId(ids.take()?);
    content.processors.push(Processor {
        id: output,
        group: Some(master_group),
        kind: ProcessorKind::Output,
    });
    content.edges.push(PipelineEdge {
        id: EdgeId(ids.take()?),
        from: (master_strip, PortId::new("audio_out")),
        to: (output, PortId::new("audio_in")),
        role: auris_core::pipeline::EdgeRole::Main,
    });

    // Reify legacy insert order and routing as ordered native edges. Port names are stable
    // semantic roles; capability resolution may later reject an unavailable plugin port, but the
    // authored route itself is never silently dropped.
    for track in &project.tracks {
        let strip = lookup(&strips, track.id)?;
        let input = lookup(&inputs, track.id)?;
        let mut previous = Some(input);
        if let Some(source) = source_nodes.get(&track.id)
            && let Some(instrument) = singers
                .get(&track.id)
                .copied()
                .or_else(|| instruments.get(&track.id).copied())
        {
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (*source, PortId::new("events_out")),
                to: (instrument, PortId::new("events_in")),
                role: auris_core::pipeline::EdgeRole::Events,
            });
        }
        if let Some(instrument) = singers
            .get(&track.id)
            .copied()
            .or_else(|| instruments.get(&track.id).copied())
        {
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (instrument, PortId::new("audio_out")),
                to: (input, PortId::new("audio_in")),
                role: auris_core::pipeline::EdgeRole::Main,
            });
        }
        if let (Some(source), Some(guide), Some(singer)) = (
            source_nodes.get(&track.id),
            instruments.get(&track.id),
            singers.get(&track.id),
        ) {
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (*source, PortId::new("events_out")),
                to: (*guide, PortId::new("events_in")),
                role: auris_core::pipeline::EdgeRole::Events,
            });
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (*guide, PortId::new("audio_out")),
                to: (*singer, PortId::new("guide_in")),
                role: auris_core::pipeline::EdgeRole::Main,
            });
        }
        for effect in &track.mixer.effects {
            let effect_node = lookup(&effects, effect.id)?;
            if let Some(from) = previous {
                content.edges.push(PipelineEdge {
                    id: EdgeId(ids.take()?),
                    from: (from, PortId::new("audio_out")),
                    to: (effect_node, PortId::new("audio_in")),
                    role: auris_core::pipeline::EdgeRole::Main,
                });
            }
            previous = Some(effect_node);
        }
        if let Some(from) = previous {
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (from, PortId::new("audio_out")),
                to: (strip, PortId::new("audio_in")),
                role: auris_core::pipeline::EdgeRole::Main,
            });
        }
        for effect in &track.mixer.effects {
            if let Some(sidechain) = effect.sidechain {
                let source_strip = lookup(&strips, sidechain)?;
                content.edges.push(PipelineEdge {
                    id: EdgeId(ids.take()?),
                    from: (source_strip, PortId::new("audio_out")),
                    to: (lookup(&effects, effect.id)?, PortId::new("sidechain_in")),
                    role: auris_core::pipeline::EdgeRole::Sidechain,
                });
            }
        }
        for output in &track.outputs {
            let destination = match output.bus() {
                Some(bus) => lookup(&inputs, bus)?,
                None => master_input,
            };
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (strip, PortId::new("audio_out")),
                to: (destination, PortId::new("audio_in")),
                role: auris_core::pipeline::EdgeRole::Main,
            });
        }
        for send in &track.sends {
            let destination = lookup(&inputs, send.target)?;
            let source_port = if send.pre_fader {
                "pre_fader"
            } else {
                "audio_out"
            };
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (strip, PortId::new(source_port)),
                to: (lookup(&sends, send.id)?, PortId::new("audio_in")),
                role: auris_core::pipeline::EdgeRole::Main,
            });
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (lookup(&sends, send.id)?, PortId::new("audio_out")),
                to: (destination, PortId::new("audio_in")),
                role: auris_core::pipeline::EdgeRole::Main,
            });
        }
    }
    for effect in &project.master.effects {
        if let Some(source) = effect.sidechain {
            content.edges.push(PipelineEdge {
                id: EdgeId(ids.take()?),
                from: (lookup(&strips, source)?, PortId::new("audio_out")),
                to: (lookup(&effects, effect.id)?, PortId::new("sidechain_in")),
                role: auris_core::pipeline::EdgeRole::Sidechain,
            });
        }
    }
    content.next_id = ids.next;
    let external: Vec<_> = project
        .audio_sources
        .keys()
        .map(|id| id.0)
        .chain(project.soundfonts.keys().map(|id| id.0))
        .collect();
    let targets = {
        let commands = content.command_bindings(&external, limits)?;
        project
            .automation
            .lanes()
            .iter()
            .enumerate()
            .map(|(index, lane)| {
                if builtin_domain(lane.target).is_none() && lane.key().is_none_or(str::is_empty) {
                    return Err(PipelineConversionError::MissingPluginKey { index });
                }
                Ok(commands.parameter(lane.target, lane.key())?)
            })
            .collect::<Result<Vec<_>, PipelineConversionError>>()?
    };

    // Preserve every legacy automation curve. Built-in mixer domains are canonical; plugin
    // domains remain unresolved until a capability provider supplies their descriptor.
    for (lane, target) in project.automation.lanes().iter().zip(targets) {
        let automation_id = auris_core::pipeline::content::AutomationId(ids.take()?);
        let domain = builtin_domain(lane.target);
        let document = match domain {
            Some(domain) => auris_core::pipeline::content::AutomationDocument::from_lane(
                automation_id,
                domain,
                lane,
            )?,
            None => auris_core::pipeline::content::AutomationDocument::from_lane_unresolved(
                automation_id,
                lane,
            )?,
        };
        let node = NodeId(ids.take()?);
        content.automation.push(document);
        content.processors.push(Processor {
            id: node,
            group: None,
            kind: ProcessorKind::Automation(automation_id),
        });
        content
            .automation_bindings
            .push(auris_core::pipeline::content::AutomationBinding {
                id: EdgeId(ids.take()?),
                source: node,
                target,
                enabled: true,
            });
    }
    content.next_id = ids.next;

    let context = NativeProjectContext {
        source_format_version: project.format_version,
        saved_by: project.saved_by.clone(),
        name: project.name.clone(),
        sample_rate: project.sample_rate,
        tempo_map: project.tempo_map.clone(),
        signatures: project.signatures.clone(),
        harmony: project.harmony.clone(),
        sections: project.sections.clone(),
        audio_sources: project.audio_sources.clone(),
        soundfonts: project.soundfonts.clone(),
        loop_region: project.loop_region,
        loop_enabled: project.loop_enabled,
        punch_region: project.punch_region,
        punch_enabled: project.punch_enabled,
        metronome: project.metronome,
        count_in_bars: project.count_in_bars,
        grid: project.grid,
        song_spec: project.song_spec.clone(),
        creation_brief: project.creation_brief.clone(),
        composition_baseline: project.composition_baseline.clone(),
        transcription_provenance: project.transcription_provenance.clone(),
        mixer_graph_view: project.mixer_graph_view.clone(),
        source_next_id: project.next_id(),
    };
    let track_presentation = project
        .tracks
        .iter()
        .map(|track| TrackPresentation {
            id: track.id,
            name: track.name.clone(),
            color: track.color,
            height: track.height,
            kind: NativeTrackKind::from(&track.kind),
        })
        .collect();
    let document = NativeDocument {
        context,
        track_presentation,
        content,
    };
    document.validate_ownership(limits)?;
    Ok(document)
}

fn preflight(project: &Project, limits: ContentLimits) -> Result<u64, PipelineConversionError> {
    bound("tracks", project.tracks.len(), limits.tracks)?;
    bound(
        "automations",
        project.automation.lanes().len(),
        limits.automations,
    )?;
    let sources = project
        .tracks
        .iter()
        .filter(|track| !track.kind.is_bus())
        .count();
    let clips = project
        .tracks
        .iter()
        .filter_map(|track| track.kind.note_clips())
        .map(Vec::len)
        .fold(0usize, usize::saturating_add)
        .saturating_add(
            project
                .tracks
                .iter()
                .filter_map(|track| track.kind.as_audio())
                .map(|track| track.clips.len())
                .fold(0usize, usize::saturating_add),
        );
    let stages = project
        .tracks
        .iter()
        .filter_map(|track| track.kind.note_clips())
        .flat_map(|clips| clips.iter())
        .map(|clip| clip.transforms.len())
        .fold(0usize, usize::saturating_add);
    let effects = project
        .tracks
        .iter()
        .map(|track| track.mixer.effects.len())
        .fold(project.master.effects.len(), usize::saturating_add);
    let sends = project
        .tracks
        .iter()
        .map(|track| track.sends.len())
        .fold(0usize, usize::saturating_add);
    let automations = project.automation.lanes().len();
    let points = project
        .automation
        .lanes()
        .iter()
        .map(|lane| lane.points().len())
        .fold(0usize, usize::saturating_add);
    let instruments = project
        .tracks
        .iter()
        .filter(|track| track.kind.note_clips().is_some())
        .count();
    let singers = project
        .tracks
        .iter()
        .filter(|track| matches!(track.kind, TrackKind::Singer(_)))
        .count();
    let processors = 3usize
        .saturating_add(project.tracks.len().saturating_mul(2))
        .saturating_add(sources)
        .saturating_add(instruments)
        .saturating_add(singers)
        .saturating_add(effects)
        .saturating_add(sends)
        .saturating_add(automations);
    let outputs = project
        .tracks
        .iter()
        .map(|track| track.outputs.len())
        .fold(0usize, usize::saturating_add);
    let sidechains = project
        .master
        .effects
        .iter()
        .chain(project.tracks.iter().flat_map(|track| &track.mixer.effects))
        .filter(|effect| effect.sidechain.is_some())
        .count();
    let source_edges = project
        .tracks
        .iter()
        .map(|track| match track.kind {
            TrackKind::Instrument(_) | TrackKind::Drum(_) => 2usize,
            TrackKind::Singer(_) => 4,
            TrackKind::Audio(_) => 1,
            TrackKind::Bus => 0,
        })
        .fold(0usize, usize::saturating_add);
    let edges = effects
        .saturating_add(project.tracks.len())
        .saturating_add(2)
        .saturating_add(source_edges)
        .saturating_add(outputs)
        .saturating_add(sends.saturating_mul(2))
        .saturating_add(sidechains);
    let groups = project.tracks.len().saturating_add(1);
    let external_ids = project
        .audio_sources
        .len()
        .saturating_add(project.soundfonts.len());
    for (name, found, limit) in [
        ("sources", sources, limits.sources),
        ("clips", clips, limits.clips),
        ("stages", stages, limits.stages),
        ("processors", processors, limits.processors),
        ("groups", groups, limits.groups),
        ("tracks", project.tracks.len(), limits.tracks),
        ("external_ids", external_ids, limits.external_ids),
        ("automations", automations, limits.automations),
        ("automation_points", points, limits.automation_points),
        ("edges", edges, limits.edges),
        ("bindings", automations, limits.bindings),
    ] {
        bound(name, found, limit)?;
    }
    if let Some(id) = project.conflicting_object_id() {
        return Err(PipelineConversionError::InvalidLegacyId(id));
    }
    if !project.routing_is_valid() {
        return Err(PipelineConversionError::InvalidLegacyRouting);
    }
    let start = project
        .allocation_start()
        .ok_or(PipelineConversionError::IdExhausted)?;
    let required = [
        groups,
        sources,
        stages,
        processors,
        edges,
        automations,
        automations,
    ]
    .into_iter()
    .try_fold(0u64, |sum, count| {
        sum.checked_add(u64::try_from(count).ok()?)
    })
    .ok_or(PipelineConversionError::IdExhausted)?;
    start
        .checked_add(required)
        .ok_or(PipelineConversionError::IdExhausted)?;
    Ok(start)
}

fn bound(name: &'static str, count: usize, limit: usize) -> Result<(), PipelineConversionError> {
    if count > limit {
        Err(auris_core::pipeline::content::ContentError::Limit(name).into())
    } else {
        Ok(())
    }
}

fn lookup<K: Ord>(
    map: &std::collections::BTreeMap<K, NodeId>,
    key: K,
) -> Result<NodeId, PipelineConversionError> {
    map.get(&key)
        .copied()
        .ok_or(PipelineConversionError::InvalidLegacyRouting)
}

fn strip_state(strip: &auris_core::project::MixerStrip) -> StripState {
    StripState {
        gain_db: strip.gain_db,
        pan: strip.pan,
        mute: strip.mute,
        solo: strip.solo,
        target_lufs: strip.target_lufs,
    }
}

fn builtin_domain(target: ParamTarget) -> Option<ControlDomain> {
    let descriptor = auris_core::param::mixer_descriptor(target)?;
    Some(ControlDomain::Native {
        unit: descriptor.unit,
        min: descriptor.min,
        max: descriptor.max,
    })
}
