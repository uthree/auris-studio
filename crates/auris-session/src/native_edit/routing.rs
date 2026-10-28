//! Identity based edits for native signal routing.
//!
//! Routing is stored as explicit processor edges.  These helpers keep the legacy mixer
//! operations addressable by stable track and send identities while leaving unrelated edges
//! untouched.  The editor applies them to a private candidate, so a failed route never consumes
//! an identity or leaves a half-updated graph.

use std::collections::BTreeSet;

use auris_core::pipeline::content::{
    ContentError, ContentLimits, ContentRef, GroupId, PipelineContent, Processor, ProcessorKind,
};
use auris_core::pipeline::{EdgeId, EdgeRole, NodeId, PipelineEdge, PortId};
use auris_core::{SendId, TrackId};

use super::{NativeCreated, NativeEditError, allocate};
use auris_core::pipeline::document::NativeDocument;

/// A routing edit addressed by persistent native identities.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum NativeRoutingEdit {
    /// Adds a send from a track strip to a processing group's input sum.
    AddSend {
        /// Track whose strip is the send source.
        track: TrackId,
        /// Destination processing group.
        destination: GroupId,
        /// Initial send gain in decibels.
        level_db: f32,
        /// Whether the send taps the strip before its fader.
        #[serde(default)]
        pre_fader: bool,
    },
    /// Changes one send's static gain while retaining its node and connection identities.
    SetSendLevel {
        /// Existing send identity.
        send: SendId,
        /// New gain in decibels.
        level_db: f32,
    },
    /// Changes a send destination while retaining its send node and edge identity.
    SetSendDestination {
        /// Existing send identity.
        send: SendId,
        /// New destination processing group.
        destination: GroupId,
    },
    /// Changes the tap point of one send while retaining its node and edge identities.
    SetSendPreFader {
        /// Existing send identity.
        send: SendId,
        /// Whether the send taps before the source strip fader.
        pre_fader: bool,
    },
    /// Removes a send processor and all of its incident signal edges.
    RemoveSend {
        /// Existing send identity.
        send: SendId,
    },
    /// Replaces the track strip's primary output route with a destination group's input sum.
    SetMainRoute {
        /// Track whose strip is routed.
        track: TrackId,
        /// Destination processing group.
        destination: GroupId,
    },
}

/// Applies one routing operation to a native document candidate.
pub(super) fn apply(
    document: &mut NativeDocument,
    edit: &NativeRoutingEdit,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    match edit {
        NativeRoutingEdit::AddSend {
            track,
            destination,
            level_db,
            pre_fader,
        } => add_send(
            document,
            *track,
            *destination,
            *level_db,
            *pre_fader,
            limits,
        ),
        NativeRoutingEdit::SetSendLevel { send, level_db } => {
            set_send_level(document, *send, *level_db)
        }
        NativeRoutingEdit::SetSendDestination { send, destination } => {
            set_send_destination(document, *send, *destination)
        }
        NativeRoutingEdit::SetSendPreFader { send, pre_fader } => {
            set_send_pre_fader(document, *send, *pre_fader)
        }
        NativeRoutingEdit::RemoveSend { send } => remove_send(document, *send),
        NativeRoutingEdit::SetMainRoute { track, destination } => {
            set_main_route(document, *track, *destination)
        }
    }
}

fn add_send(
    document: &mut NativeDocument,
    track: TrackId,
    destination: GroupId,
    level_db: f32,
    pre_fader: bool,
    limits: ContentLimits,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    validate_level(level_db)?;
    if document.content.edges.len().saturating_add(2) > limits.edges {
        return Err(ContentError::Limit("edges").into());
    }
    let source = strip_for_track(&document.content, track)?;
    let destination_sum = sum_for_group(&document.content, destination)?;
    let source_group = source_group(&document.content, source)?;
    if source_group == destination {
        return Err(NativeEditError::Invalid(
            "a send destination must differ from its source group".into(),
        ));
    }
    ensure_acyclic(&document.content, destination_sum, source)?;
    if document.content.processors.iter().any(|processor| {
        let ProcessorKind::Send { .. } = processor.kind else {
            return false;
        };
        let incoming = document.content.edges.iter().any(|edge| {
            edge.from.0 == source && edge.to.0 == processor.id && edge.role == EdgeRole::Main
        });
        let outgoing = document.content.edges.iter().any(|edge| {
            edge.from.0 == processor.id
                && edge.to.0 == destination_sum
                && edge.role == EdgeRole::Main
        });
        incoming && outgoing
    }) {
        return Err(NativeEditError::Invalid(
            "the requested send route already exists".into(),
        ));
    }
    if document.content.processors.len() >= limits.processors {
        return Err(ContentError::Limit("processors").into());
    }
    let group = source_group;
    let node = NodeId(allocate(document)?);
    let send = SendId(allocate(document)?);
    let first_edge = EdgeId(allocate(document)?);
    let second_edge = EdgeId(allocate(document)?);
    document.content.processors.push(Processor {
        id: node,
        group: Some(group),
        kind: ProcessorKind::Send { send, level_db },
    });
    let source_port = if pre_fader { "pre_fader" } else { "audio_out" };
    document.content.edges.extend([
        PipelineEdge {
            id: first_edge,
            from: (source, PortId::new(source_port)),
            to: (node, PortId::new("audio_in")),
            role: EdgeRole::Main,
        },
        PipelineEdge {
            id: second_edge,
            from: (node, PortId::new("audio_out")),
            to: (destination_sum, PortId::new("audio_in")),
            role: EdgeRole::Main,
        },
    ]);
    Ok(vec![
        NativeCreated::Send(send),
        NativeCreated::Node(node),
        NativeCreated::Edge(first_edge),
        NativeCreated::Edge(second_edge),
    ])
}

fn set_send_pre_fader(
    document: &mut NativeDocument,
    send: SendId,
    pre_fader: bool,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let node = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(processor.kind, ProcessorKind::Send { send: id, .. } if id == send))
        .map(|processor| processor.id)
        .ok_or(ContentError::Missing(ContentRef::Send(send)))?;
    let edge_id = send_input_edge(&document.content, node)?;
    let edge = document
        .content
        .edges
        .iter_mut()
        .find(|edge| edge.id == edge_id)
        .ok_or(ContentError::Missing(ContentRef::Edge(edge_id)))?;
    edge.from.1 = PortId::new(if pre_fader { "pre_fader" } else { "audio_out" });
    Ok(Vec::new())
}

fn set_send_level(
    document: &mut NativeDocument,
    send: SendId,
    level_db: f32,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    validate_level(level_db)?;
    let processor = document
        .content
        .processors
        .iter_mut()
        .find(|processor| matches!(processor.kind, ProcessorKind::Send { send: id, .. } if id == send))
        .ok_or(ContentError::Missing(ContentRef::Send(send)))?;
    let ProcessorKind::Send {
        level_db: current, ..
    } = &mut processor.kind
    else {
        unreachable!("send lookup guarantees the processor kind");
    };
    *current = level_db;
    Ok(Vec::new())
}

fn remove_send(
    document: &mut NativeDocument,
    send: SendId,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let node = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(processor.kind, ProcessorKind::Send { send: id, .. } if id == send))
        .map(|processor| processor.id)
        .ok_or(ContentError::Missing(ContentRef::Send(send)))?;
    document
        .content
        .processors
        .retain(|processor| processor.id != node);
    document
        .content
        .edges
        .retain(|edge| edge.from.0 != node && edge.to.0 != node);
    document
        .content
        .automation_bindings
        .retain(|binding| binding.target.node != node);
    Ok(Vec::new())
}

fn set_send_destination(
    document: &mut NativeDocument,
    send: SendId,
    destination: GroupId,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let node = document
        .content
        .processors
        .iter()
        .find(|processor| matches!(processor.kind, ProcessorKind::Send { send: id, .. } if id == send))
        .map(|processor| processor.id)
        .ok_or(ContentError::Missing(ContentRef::Send(send)))?;
    let destination_sum = sum_for_group(&document.content, destination)?;
    let input_edge = send_input_edge(&document.content, node)?;
    let source = document
        .content
        .edges
        .iter()
        .find(|edge| edge.id == input_edge)
        .map(|edge| edge.from.0)
        .ok_or(ContentError::Missing(ContentRef::Edge(input_edge)))?;
    let source_group = source_group(&document.content, source)?;
    if source_group == destination {
        return Err(NativeEditError::Invalid(
            "a send destination must differ from its source group".into(),
        ));
    }
    ensure_acyclic(&document.content, destination_sum, source)?;
    let outgoing: Vec<_> = document
        .content
        .edges
        .iter()
        .filter(|edge| {
            edge.from.0 == node
                && edge.from.1 == PortId::new("audio_out")
                && edge.to.1 == PortId::new("audio_in")
                && edge.role == EdgeRole::Main
        })
        .map(|edge| (edge.id, edge.to.0))
        .collect();
    let (edge_id, current_destination) = *outgoing
        .first()
        .ok_or_else(|| NativeEditError::Invalid("send has no output route".into()))?;
    if outgoing.len() > 1 {
        return Err(NativeEditError::Invalid(
            "send has multiple output routes".into(),
        ));
    }
    if current_destination == destination_sum {
        return Ok(Vec::new());
    }
    if document.content.edges.iter().any(|candidate| {
        document.content.processors.iter().any(|processor| {
            processor.id != node
                && matches!(processor.kind, ProcessorKind::Send { .. })
                && candidate.from.0 == source
                && candidate.to.0 == processor.id
                && candidate.role == EdgeRole::Main
                && document.content.edges.iter().any(|other| {
                    other.from.0 == processor.id
                        && other.to.0 == destination_sum
                        && other.role == EdgeRole::Main
                })
        })
    }) {
        return Err(NativeEditError::Invalid(
            "the requested send route already exists".into(),
        ));
    }
    let edge = document
        .content
        .edges
        .iter_mut()
        .find(|edge| edge.id == edge_id)
        .ok_or(ContentError::Missing(ContentRef::Edge(edge_id)))?;
    edge.to = (destination_sum, PortId::new("audio_in"));
    Ok(Vec::new())
}

fn set_main_route(
    document: &mut NativeDocument,
    track: TrackId,
    destination: GroupId,
) -> Result<Vec<NativeCreated>, NativeEditError> {
    let source = strip_for_track(&document.content, track)?;
    let destination_sum = sum_for_group(&document.content, destination)?;
    let source_group = source_group(&document.content, source)?;
    if source_group == destination {
        return Err(NativeEditError::Invalid(
            "a main route must leave its source group".into(),
        ));
    }
    ensure_acyclic(&document.content, destination_sum, source)?;
    let old: Vec<_> = document
        .content
        .edges
        .iter()
        .filter(|edge| {
            edge.from.0 == source
                && edge.from.1 == PortId::new("audio_out")
                && matches!(
                    document
                        .content
                        .processors
                        .iter()
                        .find(|processor| processor.id == edge.to.0)
                        .map(|processor| &processor.kind),
                    Some(ProcessorKind::Sum)
                )
                && edge.role == EdgeRole::Main
        })
        .map(|edge| edge.to.0)
        .collect();
    if old.len() == 1 && old[0] == destination_sum {
        return Ok(Vec::new());
    }
    if old.len() != 1 {
        return Err(NativeEditError::Invalid(
            "a strip must have exactly one primary output route".into(),
        ));
    }
    let edge_index = document
        .content
        .edges
        .iter()
        .position(|edge| {
            edge.from.0 == source && edge.to.0 == old[0] && edge.role == EdgeRole::Main
        })
        .ok_or_else(|| NativeEditError::Invalid("primary output edge disappeared".into()))?;
    let edge = &mut document.content.edges[edge_index];
    edge.to = (destination_sum, PortId::new("audio_in"));
    Ok(Vec::new())
}

fn validate_level(level_db: f32) -> Result<(), NativeEditError> {
    let descriptor = auris_core::param::MixerControl::Send.descriptor();
    if !level_db.is_finite() || !(descriptor.min..=descriptor.max).contains(&level_db) {
        return Err(NativeEditError::Invalid(format!(
            "send level must be finite and within {}..={} dB",
            descriptor.min, descriptor.max
        )));
    }
    Ok(())
}

fn send_input_edge(content: &PipelineContent, send: NodeId) -> Result<EdgeId, NativeEditError> {
    let candidates: Vec<_> = content
        .edges
        .iter()
        .filter(|edge| {
            edge.to.0 == send
                && edge.to.1 == PortId::new("audio_in")
                && edge.role == EdgeRole::Main
                && (edge.from.1 == PortId::new("audio_out")
                    || edge.from.1 == PortId::new("pre_fader"))
                && content.processors.iter().any(|processor| {
                    processor.id == edge.from.0 && matches!(processor.kind, ProcessorKind::Strip(_))
                })
        })
        .map(|edge| edge.id)
        .collect();
    match candidates.as_slice() {
        [edge] => Ok(*edge),
        [] => Err(NativeEditError::Invalid(
            "send has no valid strip input route".into(),
        )),
        _ => Err(NativeEditError::Invalid(
            "send has multiple valid strip input routes".into(),
        )),
    }
}

fn strip_for_track(content: &PipelineContent, track: TrackId) -> Result<NodeId, NativeEditError> {
    let binding = content
        .tracks
        .iter()
        .find(|binding| binding.track == track)
        .ok_or(ContentError::Missing(ContentRef::Track(track)))?;
    content
        .groups
        .iter()
        .find(|group| group.id == binding.group)
        .map(|group| group.strip)
        .ok_or(ContentError::Missing(ContentRef::Group(binding.group)).into())
}

fn source_group(content: &PipelineContent, node: NodeId) -> Result<GroupId, NativeEditError> {
    content
        .processors
        .iter()
        .find(|processor| processor.id == node)
        .and_then(|processor| processor.group)
        .ok_or(ContentError::Missing(ContentRef::Node(node)).into())
}

fn sum_for_group(content: &PipelineContent, group: GroupId) -> Result<NodeId, NativeEditError> {
    if !content.groups.iter().any(|entry| entry.id == group) {
        return Err(ContentError::Missing(ContentRef::Group(group)).into());
    }
    let mut sums = content.processors.iter().filter(|processor| {
        processor.group == Some(group) && matches!(processor.kind, ProcessorKind::Sum)
    });
    let sum = sums
        .next()
        .ok_or_else(|| NativeEditError::Invalid("processing group has no input sum".into()))?;
    if sums.next().is_some() {
        return Err(NativeEditError::Invalid(
            "processing group has an ambiguous input sum".into(),
        ));
    }
    Ok(sum.id)
}

fn ensure_acyclic(
    content: &PipelineContent,
    start: NodeId,
    target: NodeId,
) -> Result<(), NativeEditError> {
    let mut todo = vec![start];
    let mut seen = BTreeSet::new();
    while let Some(node) = todo.pop() {
        if node == target {
            return Err(NativeEditError::Invalid(
                "routing edit would create a cycle".into(),
            ));
        }
        if !seen.insert(node) {
            continue;
        }
        todo.extend(
            content
                .edges
                .iter()
                .filter(|edge| edge.from.0 == node)
                .map(|edge| edge.to.0),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Edit;
    use crate::native_edit::{
        NativeDocumentEditor, NativeEdit, NativeEditOwner, NativeEditReceipt, NativeTrackEdit,
        NativeTrackKindSpec,
    };
    use auris_core::pipeline::content::{ContentLimits, PluginInstance};
    use auris_core::plugin::PluginState;
    use auris_core::{Project, TrackId};

    fn limits() -> ContentLimits {
        ContentLimits {
            sources: 64,
            clips: 128,
            stages: 512,
            processors: 512,
            groups: 128,
            tracks: 128,
            edges: 1024,
            external_ids: 128,
            automations: 128,
            automation_points: 1024,
            bindings: 128,
        }
    }

    fn editor() -> NativeDocumentEditor {
        let project = Project::new("routing", 48_000.0);
        NativeDocumentEditor::new(
            auris_io::project_to_native(&project, limits()).unwrap(),
            limits(),
        )
        .unwrap()
    }

    fn add_track(editor: &mut NativeDocumentEditor, kind: NativeTrackKindSpec) -> TrackId {
        let receipt = editor
            .apply(
                editor.revision().document,
                NativeEditOwner::Human,
                Edit::AddInstrumentTrack,
                &[NativeEdit::Track(NativeTrackEdit::Add {
                    name: "route".into(),
                    kind,
                })],
            )
            .unwrap();
        receipt
            .created
            .into_iter()
            .find_map(|created| match created {
                NativeCreated::Track(track) => Some(track),
                _ => None,
            })
            .unwrap()
    }

    fn bus(editor: &NativeDocumentEditor, track: TrackId) -> GroupId {
        editor
            .document()
            .content
            .tracks
            .iter()
            .find(|binding| binding.track == track)
            .unwrap()
            .group
    }

    fn route(editor: &mut NativeDocumentEditor, operation: NativeRoutingEdit) -> NativeEditReceipt {
        editor
            .apply(
                editor.revision().document,
                NativeEditOwner::Human,
                Edit::AddSend,
                &[NativeEdit::Routing(operation)],
            )
            .unwrap()
    }

    fn audio_kind() -> NativeTrackKindSpec {
        NativeTrackKindSpec::Audio
    }

    fn bus_kind() -> NativeTrackKindSpec {
        NativeTrackKindSpec::Bus
    }

    #[test]
    fn add_remove_send_undo_redo_preserves_stable_id() {
        let mut editor = editor();
        let source = add_track(&mut editor, audio_kind());
        let destination = add_track(&mut editor, bus_kind());
        let destination_group = bus(&editor, destination);
        let receipt = route(
            &mut editor,
            NativeRoutingEdit::AddSend {
                track: source,
                destination: destination_group,
                level_db: -6.0,
                pre_fader: false,
            },
        );
        let send = receipt
            .created
            .iter()
            .find_map(|created| match created {
                NativeCreated::Send(send) => Some(*send),
                _ => None,
            })
            .unwrap();
        let node = editor
            .document()
            .content
            .processors
            .iter()
            .find(|processor| matches!(processor.kind, ProcessorKind::Send { send: id, .. } if id == send))
            .unwrap()
            .id;
        route(&mut editor, NativeRoutingEdit::RemoveSend { send });
        assert!(
            !editor
                .document()
                .content
                .processors
                .iter()
                .any(|item| item.id == node)
        );
        editor.undo(editor.revision().document).unwrap();
        assert!(
            editor
                .document()
                .content
                .processors
                .iter()
                .any(|item| item.id == node)
        );
        assert!(editor.document().content.processors.iter().any(|item| {
            matches!(item.kind, ProcessorKind::Send { send: id, .. } if id == send)
        }));
        editor.redo(editor.revision().document).unwrap();
        assert!(
            !editor
                .document()
                .content
                .processors
                .iter()
                .any(|item| item.id == node)
        );
    }

    #[test]
    fn duplicate_send_is_atomic_and_preserves_redo_and_high_water() {
        let mut editor = editor();
        let source = add_track(&mut editor, audio_kind());
        let destination = add_track(&mut editor, bus_kind());
        let destination_group = bus(&editor, destination);
        route(
            &mut editor,
            NativeRoutingEdit::AddSend {
                track: source,
                destination: destination_group,
                level_db: 0.0,
                pre_fader: false,
            },
        );
        let track_name = editor
            .document()
            .track_presentation
            .iter()
            .find(|presentation| presentation.id == source)
            .map(|presentation| presentation.name.clone())
            .unwrap();
        editor
            .apply(
                editor.revision().document,
                NativeEditOwner::Human,
                Edit::RenameTrack,
                &[NativeEdit::RenameTrack {
                    track: source,
                    name: "temporary".into(),
                }],
            )
            .unwrap();
        editor.undo(editor.revision().document).unwrap();
        assert!(editor.can_redo());
        assert_eq!(
            editor
                .document()
                .track_presentation
                .iter()
                .find(|presentation| presentation.id == source)
                .map(|presentation| presentation.name.as_str()),
            Some(track_name.as_str())
        );
        let before = editor.document().clone();
        let revision = editor.revision();
        let result = editor.apply(
            revision.document,
            NativeEditOwner::Human,
            Edit::AddSend,
            &[NativeEdit::Routing(NativeRoutingEdit::AddSend {
                track: source,
                destination: destination_group,
                level_db: 0.0,
                pre_fader: false,
            })],
        );
        assert!(result.is_err());
        assert_eq!(editor.document(), &before);
        assert_eq!(editor.revision(), revision);
        assert!(editor.can_redo());
    }

    #[test]
    fn cycle_rejection_does_not_change_document_or_counter() {
        let mut editor = editor();
        let first = add_track(&mut editor, audio_kind());
        let second = add_track(&mut editor, bus_kind());
        let second_group = bus(&editor, second);
        route(
            &mut editor,
            NativeRoutingEdit::SetMainRoute {
                track: first,
                destination: second_group,
            },
        );
        let before = editor.document().clone();
        let revision = editor.revision();
        let first_group = bus(&editor, first);
        let result = editor.apply(
            revision.document,
            NativeEditOwner::Human,
            Edit::AddSend,
            &[NativeEdit::Routing(NativeRoutingEdit::SetMainRoute {
                track: second,
                destination: first_group,
            })],
        );
        assert!(result.is_err());
        assert_eq!(editor.document(), &before);
        assert_eq!(editor.revision(), revision);
    }

    #[test]
    fn main_route_changes_only_primary_output_edge() {
        let mut editor = editor();
        let source = add_track(&mut editor, audio_kind());
        let first = add_track(&mut editor, bus_kind());
        let second = add_track(&mut editor, bus_kind());
        let first_group = bus(&editor, first);
        let second_group = bus(&editor, second);
        route(
            &mut editor,
            NativeRoutingEdit::AddSend {
                track: source,
                destination: first_group,
                level_db: -3.0,
                pre_fader: true,
            },
        );
        route(
            &mut editor,
            NativeRoutingEdit::SetMainRoute {
                track: source,
                destination: second_group,
            },
        );
        let strip = editor
            .document()
            .content
            .tracks
            .iter()
            .find(|binding| binding.track == source)
            .and_then(|binding| {
                editor
                    .document()
                    .content
                    .groups
                    .iter()
                    .find(|group| group.id == binding.group)
            })
            .unwrap()
            .strip;
        let main_edges = editor
            .document()
            .content
            .edges
            .iter()
            .filter(|edge| edge.from.0 == strip && edge.role == EdgeRole::Main)
            .count();
        assert_eq!(main_edges, 2);
    }

    #[test]
    fn send_destination_and_tap_updates_keep_edge_ids_in_one_batch() {
        let mut editor = editor();
        let source = add_track(&mut editor, audio_kind());
        let first = add_track(&mut editor, bus_kind());
        let second = add_track(&mut editor, bus_kind());
        let first_group = bus(&editor, first);
        let second_group = bus(&editor, second);
        let receipt = route(
            &mut editor,
            NativeRoutingEdit::AddSend {
                track: source,
                destination: first_group,
                level_db: -3.0,
                pre_fader: true,
            },
        );
        let send = receipt
            .created
            .iter()
            .find_map(|created| match created {
                NativeCreated::Send(send) => Some(*send),
                _ => None,
            })
            .unwrap();
        let node = editor
            .document()
            .content
            .processors
            .iter()
            .find(|processor| matches!(processor.kind, ProcessorKind::Send { send: id, .. } if id == send))
            .unwrap()
            .id;
        let before_edges: Vec<_> = editor
            .document()
            .content
            .edges
            .iter()
            .filter(|edge| edge.from.0 == node || edge.to.0 == node)
            .map(|edge| edge.id)
            .collect();
        let destination_sum = sum_for_group(&editor.document().content, second_group).unwrap();
        let receipt = editor
            .apply(
                editor.revision().document,
                NativeEditOwner::Human,
                Edit::AddSend,
                &[
                    NativeEdit::Routing(NativeRoutingEdit::SetSendDestination {
                        send,
                        destination: second_group,
                    }),
                    NativeEdit::Routing(NativeRoutingEdit::SetSendPreFader {
                        send,
                        pre_fader: false,
                    }),
                ],
            )
            .unwrap();
        assert!(receipt.changed);
        let after_edges: Vec<_> = editor
            .document()
            .content
            .edges
            .iter()
            .filter(|edge| edge.from.0 == node || edge.to.0 == node)
            .collect();
        assert_eq!(after_edges.len(), 2);
        assert_eq!(
            after_edges.iter().map(|edge| edge.id).collect::<Vec<_>>(),
            before_edges
        );
        assert!(after_edges.iter().any(|edge| edge.to.0 == destination_sum));
        assert!(
            after_edges
                .iter()
                .any(|edge| edge.from.1 == PortId::new("audio_out"))
        );
    }

    #[test]
    fn main_route_requires_one_existing_primary_sum_edge() {
        let mut source_editor = editor();
        let source = add_track(&mut source_editor, audio_kind());
        let destination = add_track(&mut source_editor, bus_kind());
        let destination_group = bus(&source_editor, destination);
        let source_group = bus(&source_editor, source);
        let source_strip = source_editor
            .document()
            .content
            .groups
            .iter()
            .find(|group| group.id == source_group)
            .unwrap()
            .strip;
        let mut missing = source_editor.document().clone();
        missing.content.edges.retain(|edge| {
            !(edge.from.0 == source_strip
                && edge.role == EdgeRole::Main
                && matches!(
                    missing
                        .content
                        .processors
                        .iter()
                        .find(|processor| processor.id == edge.to.0)
                        .map(|processor| &processor.kind),
                    Some(ProcessorKind::Sum)
                ))
        });
        let mut editor = NativeDocumentEditor::new(missing, limits()).unwrap();
        let result = editor.apply(
            editor.revision().document,
            NativeEditOwner::Human,
            Edit::AddSend,
            &[NativeEdit::Routing(NativeRoutingEdit::SetMainRoute {
                track: source,
                destination: destination_group,
            })],
        );
        assert!(result.is_err());

        let mut ambiguous = source_editor.document().clone();
        let primary = ambiguous
            .content
            .edges
            .iter()
            .find(|edge| {
                edge.from.0 == source_strip
                    && edge.role == EdgeRole::Main
                    && matches!(
                        ambiguous
                            .content
                            .processors
                            .iter()
                            .find(|processor| processor.id == edge.to.0)
                            .map(|processor| &processor.kind),
                        Some(ProcessorKind::Sum)
                    )
            })
            .cloned()
            .unwrap();
        let mut duplicate = primary;
        duplicate.id = EdgeId(ambiguous.content.next_id);
        ambiguous.content.next_id += 1;
        ambiguous.content.edges.push(duplicate);
        let mut editor = NativeDocumentEditor::new(ambiguous, limits()).unwrap();
        let result = editor.apply(
            editor.revision().document,
            NativeEditOwner::Human,
            Edit::AddSend,
            &[NativeEdit::Routing(NativeRoutingEdit::SetMainRoute {
                track: source,
                destination: destination_group,
            })],
        );
        assert!(result.is_err());
    }

    #[test]
    fn malformed_send_inputs_are_rejected_without_mutation() {
        let mut original = editor();
        let source = add_track(&mut original, audio_kind());
        let destination = add_track(&mut original, bus_kind());
        let destination_group = bus(&original, destination);
        let receipt = route(
            &mut original,
            NativeRoutingEdit::AddSend {
                track: source,
                destination: destination_group,
                level_db: -3.0,
                pre_fader: false,
            },
        );
        let send = receipt
            .created
            .iter()
            .find_map(|created| match created {
                NativeCreated::Send(send) => Some(*send),
                _ => None,
            })
            .unwrap();
        let mut malformed = original.document().clone();
        let node = malformed
            .content
            .processors
            .iter()
            .find(|processor| matches!(processor.kind, ProcessorKind::Send { send: id, .. } if id == send))
            .unwrap()
            .id;
        let input = malformed
            .content
            .edges
            .iter()
            .find(|edge| edge.to.0 == node && edge.to.1 == PortId::new("audio_in"))
            .cloned()
            .unwrap();
        let mut duplicate = input;
        duplicate.id = EdgeId(malformed.content.next_id);
        malformed.content.next_id += 1;
        malformed.content.edges.push(duplicate);
        let mut editor = NativeDocumentEditor::new(malformed, limits()).unwrap();
        let before = editor.document().clone();
        let revision = editor.revision();
        let result = editor.apply(
            revision.document,
            NativeEditOwner::Human,
            Edit::AddSend,
            &[NativeEdit::Routing(NativeRoutingEdit::SetSendPreFader {
                send,
                pre_fader: true,
            })],
        );
        assert!(result.is_err());
        assert_eq!(editor.document(), &before);
        assert_eq!(editor.revision(), revision);
    }

    #[allow(dead_code)]
    fn _plugin() -> NativeTrackKindSpec {
        NativeTrackKindSpec::Instrument(PluginInstance {
            plugin_id: "builtin:test".into(),
            state: PluginState::empty(),
            file: None,
        })
    }
}
