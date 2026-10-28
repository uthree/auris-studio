//! Native track creation and deletion, with explicit source and processor ownership.

use std::collections::BTreeSet;

use auris_core::pipeline::content::{
    ContentError, ContentLimits, ContentRef, GroupId, PluginInstance, ProcessingGroup, Processor,
    ProcessorKind, SingerInstance, SourceContent, SourceDocument, SourceDocumentId, StripState,
    TrackBinding,
};
use auris_core::pipeline::document::{NativeDocument, NativeTrackKind, TrackPresentation};
use auris_core::pipeline::{EdgeId, EdgeRole, NodeId, PipelineEdge, PortId};
use auris_core::{Color, Track, TrackId};

use super::{NativeCreated, NativeEditError, allocate};

/// Authored role and initial processors of a newly created native track.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum NativeTrackKindSpec {
    /// Audio placements with no instrument.
    Audio,
    /// A routing bus with no authored source.
    Bus,
    /// Pitched notes feeding an independent instrument instance.
    Instrument(PluginInstance),
    /// Percussion notes feeding an independent instrument instance.
    Drum(PluginInstance),
    /// Singer material with an independent guide and offline singer processor.
    Singer {
        /// Instrument used when no take is selected.
        guide: PluginInstance,
        /// Voice settings and any declared cached take.
        singer: Box<SingerInstance>,
    },
}

/// Track operations addressed by persistent identities.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum NativeTrackEdit {
    /// Appends a track with fresh source/processor identities and a route to the master sum.
    Add {
        /// Display name.
        name: String,
        /// Initial track role and processors.
        kind: NativeTrackKindSpec,
    },
    /// Deletes a track and its processing group, retaining sources still used elsewhere.
    Remove {
        /// Track identity.
        track: TrackId,
        /// Also remove its source if it has no remaining timeline or graph consumers.
        prune_source: bool,
    },
    /// Reorders presentation and conventional track bindings together.
    Move {
        /// Track identity.
        track: TrackId,
        /// New index, clamped to the existing track list.
        index: usize,
    },
}

pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativeTrackEdit,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    match edit {
        NativeTrackEdit::Add { name, kind } => add(document, name, kind, limits),
        NativeTrackEdit::Remove {
            track,
            prune_source,
        } => {
            remove(document, *track, *prune_source)?;
            Ok(Vec::new())
        }
        NativeTrackEdit::Move { track, index } => {
            let from = document
                .track_presentation
                .iter()
                .position(|entry| entry.id == *track)
                .ok_or(ContentError::Missing(ContentRef::Track(*track)))?;
            let to = (*index).min(document.track_presentation.len().saturating_sub(1));
            let presentation = document.track_presentation.remove(from);
            document.track_presentation.insert(to, presentation);
            let from = document
                .content
                .tracks
                .iter()
                .position(|entry| entry.track == *track)
                .ok_or(ContentError::Missing(ContentRef::Track(*track)))?;
            let binding = document.content.tracks.remove(from);
            document.content.tracks.insert(to, binding);
            Ok(Vec::new())
        }
    }
}

fn add(
    document: &mut NativeDocument,
    name: &str,
    spec: &NativeTrackKindSpec,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    if document.content.tracks.len() >= limits.tracks {
        return Err(ContentError::Limit("tracks").into());
    }
    let encoded =
        serde_json::to_vec(spec).map_err(|error| NativeEditError::Invalid(error.to_string()))?;
    let decoded: NativeTrackKindSpec = serde_json::from_slice(&encoded)
        .map_err(|error| NativeEditError::Invalid(error.to_string()))?;
    if decoded != *spec {
        return Err(NativeEditError::Invalid(
            "track settings cannot be stored without losing values".into(),
        ));
    }
    let master_input = sum(document, document.content.master)?;
    let (kind, color, plugin) = match spec {
        NativeTrackKindSpec::Audio => (NativeTrackKind::Audio, Color::AUDIO, None),
        NativeTrackKindSpec::Bus => (NativeTrackKind::Bus, Color::BUS, None),
        NativeTrackKindSpec::Instrument(plugin) => {
            (NativeTrackKind::Instrument, Color::INSTRUMENT, Some(plugin))
        }
        NativeTrackKindSpec::Drum(plugin) => (NativeTrackKind::Drum, Color::DRUM, Some(plugin)),
        NativeTrackKindSpec::Singer { guide, singer } => {
            if !singer.frame_hop.is_finite() || singer.frame_hop <= 0.0 {
                return Err(NativeEditError::Invalid(
                    "singer frame hop must be finite and positive".into(),
                ));
            }
            (NativeTrackKind::Singer, Color::SINGER, Some(guide))
        }
    };
    if let Some(plugin) = plugin
        && (plugin.plugin_id.is_empty()
            || plugin.state.params.values().any(|value| !value.is_finite()))
    {
        return Err(NativeEditError::Invalid(
            "instrument identity and parameter values must be valid".into(),
        ));
    }
    let track = TrackId(allocate(document)?);
    let group = GroupId(allocate(document)?);
    let strip = node(
        document,
        Some(group),
        ProcessorKind::Strip(StripState::default()),
    )?;
    let input = node(document, Some(group), ProcessorKind::Sum)?;
    let primary = plugin
        .map(|plugin| {
            node(
                document,
                Some(group),
                ProcessorKind::Instrument(plugin.clone()),
            )
        })
        .transpose()?;
    document.content.groups.push(ProcessingGroup {
        id: group,
        strip,
        primary_instrument: primary,
    });
    let mut created = vec![
        NativeCreated::Track(track),
        NativeCreated::Group(group),
        NativeCreated::Node(strip),
        NativeCreated::Node(input),
    ];
    if let Some(primary) = primary {
        created.push(NativeCreated::Node(primary));
    }
    let source = if kind != NativeTrackKind::Bus {
        let source = SourceDocumentId(allocate(document)?);
        document.content.sources.push(SourceDocument {
            id: source,
            content: if kind == NativeTrackKind::Audio {
                SourceContent::Audio(Vec::new())
            } else {
                SourceContent::Notes(Vec::new())
            },
        });
        let source_node = node(document, None, ProcessorKind::Source(source))?;
        created.extend([
            NativeCreated::Source(source),
            NativeCreated::Node(source_node),
        ]);
        match spec {
            NativeTrackKindSpec::Audio => {
                edge(
                    document,
                    source_node,
                    "audio_out",
                    input,
                    "audio_in",
                    EdgeRole::Main,
                )?;
            }
            NativeTrackKindSpec::Singer { singer, .. } => {
                let guide = primary
                    .ok_or_else(|| NativeEditError::Invalid("singer has no guide".into()))?;
                let singer = node(document, Some(group), ProcessorKind::Singer(singer.clone()))?;
                created.push(NativeCreated::Node(singer));
                edge(
                    document,
                    source_node,
                    "events_out",
                    guide,
                    "events_in",
                    EdgeRole::Events,
                )?;
                edge(
                    document,
                    source_node,
                    "events_out",
                    singer,
                    "events_in",
                    EdgeRole::Events,
                )?;
                edge(
                    document,
                    guide,
                    "audio_out",
                    singer,
                    "guide_in",
                    EdgeRole::Main,
                )?;
                edge(
                    document,
                    singer,
                    "audio_out",
                    input,
                    "audio_in",
                    EdgeRole::Main,
                )?;
            }
            NativeTrackKindSpec::Instrument(_) | NativeTrackKindSpec::Drum(_) => {
                let instrument = primary
                    .ok_or_else(|| NativeEditError::Invalid("track has no instrument".into()))?;
                edge(
                    document,
                    source_node,
                    "events_out",
                    instrument,
                    "events_in",
                    EdgeRole::Events,
                )?;
                edge(
                    document,
                    instrument,
                    "audio_out",
                    input,
                    "audio_in",
                    EdgeRole::Main,
                )?;
            }
            NativeTrackKindSpec::Bus => {}
        }
        Some(source)
    } else {
        None
    };
    edge(
        document,
        input,
        "audio_out",
        strip,
        "audio_in",
        EdgeRole::Main,
    )?;
    edge(
        document,
        strip,
        "audio_out",
        master_input,
        "audio_in",
        EdgeRole::Main,
    )?;
    document.content.tracks.push(TrackBinding {
        track,
        source,
        group,
    });
    document.track_presentation.push(TrackPresentation {
        id: track,
        name: name.into(),
        color,
        height: Track::DEFAULT_HEIGHT,
        kind,
    });
    Ok(created)
}

fn remove(
    document: &mut NativeDocument,
    track: TrackId,
    prune_source: bool,
) -> Result<(), NativeEditError> {
    let binding = document
        .content
        .tracks
        .iter()
        .find(|entry| entry.track == track)
        .cloned()
        .ok_or(ContentError::Missing(ContentRef::Track(track)))?;
    if document
        .content
        .tracks
        .iter()
        .any(|entry| entry.track != track && entry.group == binding.group)
    {
        return Err(NativeEditError::Invalid(
            "shared processing groups must be detached before deleting a track".into(),
        ));
    }
    if binding.group == document.content.master {
        return Err(ContentError::MasterRemoval.into());
    }
    let master_input = sum(document, document.content.master)?;
    let mut removed: BTreeSet<NodeId> = document
        .content
        .processors
        .iter()
        .filter(|node| node.group == Some(binding.group))
        .map(|node| node.id)
        .collect();
    // A send whose only destination disappears leaves with that route; a shared send with
    // another live destination remains. Main outputs fall back to master only when emptied.
    let sends: Vec<_> = document
        .content
        .processors
        .iter()
        .filter_map(|node| {
            if !matches!(node.kind, ProcessorKind::Send { .. }) || removed.contains(&node.id) {
                return None;
            }
            let outputs: Vec<_> = document
                .content
                .edges
                .iter()
                .filter(|edge| edge.from.0 == node.id && edge.role == EdgeRole::Main)
                .collect();
            (!outputs.is_empty() && outputs.iter().all(|edge| removed.contains(&edge.to.0)))
                .then_some(node.id)
        })
        .collect();
    removed.extend(sends);
    let redirect: BTreeSet<_> = document
        .content
        .edges
        .iter()
        .filter(|edge| {
            removed.contains(&edge.to.0)
                && !removed.contains(&edge.from.0)
                && edge.role == EdgeRole::Main
                && document.content.processors.iter().any(|node| {
                    node.id == edge.from.0 && matches!(node.kind, ProcessorKind::Strip(_))
                })
                && !document.content.edges.iter().any(|other| {
                    other.from == edge.from
                        && other.role == EdgeRole::Main
                        && !removed.contains(&other.to.0)
                })
        })
        .map(|edge| edge.from.0)
        .collect();
    document
        .content
        .edges
        .retain(|edge| !removed.contains(&edge.from.0) && !removed.contains(&edge.to.0));
    for node in redirect {
        edge(
            document,
            node,
            "audio_out",
            master_input,
            "audio_in",
            EdgeRole::Main,
        )?;
    }
    document
        .content
        .processors
        .retain(|node| !removed.contains(&node.id));
    document
        .content
        .automation_bindings
        .retain(|entry| !removed.contains(&entry.target.node));
    document
        .content
        .groups
        .retain(|group| group.id != binding.group);
    document.content.tracks.retain(|entry| entry.track != track);
    document
        .track_presentation
        .retain(|entry| entry.id != track);
    if prune_source && let Some(source) = binding.source {
        let nodes: BTreeSet<_> = document
            .content
            .processors
            .iter()
            .filter_map(|node| {
                matches!(node.kind, ProcessorKind::Source(id) if id == source).then_some(node.id)
            })
            .collect();
        let used = document
            .content
            .tracks
            .iter()
            .any(|entry| entry.source == Some(source))
            || document
                .content
                .edges
                .iter()
                .any(|edge| nodes.contains(&edge.from.0) || nodes.contains(&edge.to.0));
        if !used {
            let clips: BTreeSet<_> = document
                .content
                .sources
                .iter()
                .filter(|entry| entry.id == source)
                .flat_map(|entry| match &entry.content {
                    SourceContent::Notes(clips) => {
                        clips.iter().map(|clip| clip.id).collect::<Vec<_>>()
                    }
                    SourceContent::Audio(clips) => clips.iter().map(|clip| clip.id).collect(),
                })
                .collect();
            document
                .context
                .transcription_provenance
                .retain(|clip, _| !clips.contains(clip));
            document
                .content
                .processors
                .retain(|node| !nodes.contains(&node.id));
            document.content.sources.retain(|entry| entry.id != source);
        }
    }
    Ok(())
}

fn sum(document: &NativeDocument, group: GroupId) -> Result<NodeId, NativeEditError> {
    let mut sums = document
        .content
        .processors
        .iter()
        .filter(|node| node.group == Some(group) && matches!(node.kind, ProcessorKind::Sum));
    let node = sums
        .next()
        .ok_or_else(|| NativeEditError::Invalid("processing group has no sum".into()))?;
    if sums.next().is_some() {
        return Err(NativeEditError::Invalid(
            "processing group has an ambiguous input sum".into(),
        ));
    }
    Ok(node.id)
}

fn node(
    document: &mut NativeDocument,
    group: Option<GroupId>,
    kind: ProcessorKind,
) -> Result<NodeId, NativeEditError> {
    let id = NodeId(allocate(document)?);
    document
        .content
        .processors
        .push(Processor { id, group, kind });
    Ok(id)
}

fn edge(
    document: &mut NativeDocument,
    from: NodeId,
    output: &str,
    to: NodeId,
    input: &str,
    role: EdgeRole,
) -> Result<(), NativeEditError> {
    let id = EdgeId(allocate(document)?);
    document.content.edges.push(PipelineEdge {
        id,
        from: (from, PortId::new(output)),
        to: (to, PortId::new(input)),
        role,
    });
    Ok(())
}
