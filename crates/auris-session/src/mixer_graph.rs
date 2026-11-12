//! The audible mixer as a directed acyclic graph, shared by every frontend.
//!
//! The nodes are strips — every track, every bus and the master — and nothing else. An effect is
//! not a node: it is a row of its strip's insert chain, processed in order before the fader, and
//! a parallel branch is a bus. The edges are a strip's outputs, the sidechains that key an effect
//! from another strip, and a layer borrowing its source's notes. Editable edges use the same
//! session commands as the console and the toolbox.

use auris_core::{Destination, EffectSlotId, Tap, TrackId};

use crate::{Session, SessionError};

pub mod commands;

/// A strip in the signal path: a track, a bus, or the master (`None`).
///
/// Its inserts, fader, pan, mute and solo all belong to this one node, so the master appears in
/// the graph exactly once however many effects it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MixerNode(pub Option<TrackId>);

impl MixerNode {
    /// The master strip.
    pub const MASTER: Self = Self(None);
}

/// The operation represented by an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MixerEdgeKind {
    /// One of a track's outputs; its level and tap point belong to the document.
    ///
    /// The destination is the edge's target. Together with `tap` it is what names the output,
    /// so two edges between the same nodes are told apart by where they are taken.
    Output {
        /// Owner of the output.
        track: TrackId,
        /// Level in decibels.
        gain_db: f32,
        /// Where on the strip the copy is taken.
        tap: Tap,
    },
    /// A key signal arriving at one of the target strip's effects.
    Sidechain {
        /// The keyed effect, in the target strip's chain.
        slot: EffectSlotId,
    },
    /// A layer playing its source's notes: from the source's strip to the layer's.
    ///
    /// Notes rather than audio, so it carries no level and no meter; disconnecting it detaches
    /// the layer.
    Notes,
}

/// One directed signal connection between two strips.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerEdge {
    /// Where the signal leaves.
    pub source: MixerNode,
    /// Where the signal arrives.
    pub target: MixerNode,
    /// The role of this connection.
    pub kind: MixerEdgeKind,
}

/// Where a graph gesture lands on a strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MixerPort {
    /// The strip's summing input; a bus or the master. Receives a new output.
    Input(MixerNode),
    /// One effect's detector input inside the strip. Receives a sidechain.
    Key(MixerNode, EffectSlotId),
}

/// A document projection; reading it does not build plugins or touch the audio thread.
#[derive(Clone, Debug, PartialEq)]
pub struct MixerGraph {
    /// Track strips in document order, followed by the master strip.
    pub nodes: Vec<MixerNode>,
    /// All output, sidechain and notes connections.
    pub edges: Vec<MixerEdge>,
}

impl Session {
    /// Disconnect an exact set of editable graph edges as one undo step.
    /// Stale edges reject the complete request before history changes.
    pub fn disconnect_mixer_edges(&mut self, edges: &[MixerEdge]) -> Result<bool, SessionError> {
        let graph = self.mixer_graph();
        if edges.is_empty() || edges.iter().any(|edge| !graph.edges.contains(edge)) {
            return Ok(false);
        }
        let mut unique = Vec::new();
        for edge in edges {
            if !unique.contains(edge) {
                unique.push(*edge);
            }
        }
        self.begin_transaction(crate::Edit::ExternalChanges);
        let result = (|| {
            for edge in unique {
                match edge.kind {
                    MixerEdgeKind::Output { track, tap, .. } => {
                        self.remove_track_output(track, destination(edge.target.0), tap)?;
                    }
                    MixerEdgeKind::Sidechain { slot } => {
                        self.set_effect_sidechain(edge.target.0, slot, None)?;
                    }
                    MixerEdgeKind::Notes => {
                        if let Some(layer) = edge.target.0 {
                            self.detach_layer(layer)?;
                        }
                    }
                }
            }
            Ok(true)
        })();
        self.end_transaction();
        result
    }

    /// Describes the complete mixer signal path, including bypassed inserts and silent outputs.
    pub fn mixer_graph(&self) -> MixerGraph {
        let mut graph = MixerGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
        };
        for (owner, strip) in self
            .project()
            .tracks
            .iter()
            .map(|track| (Some(track.id), &track.mixer))
            .chain(std::iter::once((None, &self.project().master)))
        {
            let node = MixerNode(owner);
            graph.nodes.push(node);
            for slot in &strip.effects {
                if let Some(source) = slot.sidechain {
                    graph.edges.push(MixerEdge {
                        source: MixerNode(Some(source)),
                        target: node,
                        kind: MixerEdgeKind::Sidechain { slot: slot.id },
                    });
                }
            }
            if let Some(track) = owner.and_then(|id| self.project().track(id)) {
                if let Some(source) = track.kind.layer()
                    && self.project().track(source.track).is_some()
                {
                    graph.edges.push(MixerEdge {
                        source: MixerNode(Some(source.track)),
                        target: node,
                        kind: MixerEdgeKind::Notes,
                    });
                }
                for output in &track.outputs {
                    graph.edges.push(MixerEdge {
                        source: node,
                        target: MixerNode(output.bus()),
                        kind: MixerEdgeKind::Output {
                            track: track.id,
                            gain_db: output.gain_db,
                            tap: output.tap,
                        },
                    });
                }
            }
        }
        graph
    }

    /// Whether a graph gesture can connect a track's signal to this port.
    ///
    /// An input receives a new unity output after the fader. An effect's key receives a
    /// sidechain. Self connections, duplicates, stale identities and all cycles are rejected.
    pub fn can_connect_mixer(&mut self, source: TrackId, target: MixerPort) -> bool {
        let Some(track) = self.project().track(source) else {
            return false;
        };
        match target {
            MixerPort::Input(MixerNode(owner)) => {
                if let Some(bus) = owner
                    && (!self.project().track(bus).is_some_and(|t| t.kind.is_bus())
                        || self.project().routing_would_cycle(source, bus))
                {
                    return false;
                }
                let key = (destination(owner), Tap::PostFader);
                !track.outputs.iter().any(|output| output.key() == key)
            }
            MixerPort::Key(MixerNode(owner), slot) => {
                self.effect_enabled(owner, slot).is_some()
                    && self.effect_sidechain(owner, slot) != Some(source)
                    && !self.project().sidechain_would_cycle(owner, source)
                    && self.effect_wants_sidechain(owner, slot)
            }
        }
    }

    /// Applies a validated graph connection as one undoable session command.
    ///
    /// Returns `false` without changing the document or history when the gesture is invalid.
    /// Validation happens again here even when the frontend already validated a preview.
    pub fn connect_mixer(
        &mut self,
        source: TrackId,
        target: MixerPort,
    ) -> Result<bool, SessionError> {
        if !self.can_connect_mixer(source, target) {
            return Ok(false);
        }
        match target {
            MixerPort::Input(MixerNode(owner)) => {
                self.add_track_output(source, destination(owner))?
            }
            MixerPort::Key(MixerNode(owner), slot) => {
                self.set_effect_sidechain(owner, slot, Some(source))?
            }
        }
        Ok(true)
    }

    /// The strip holding an effect slot, or `None` when no strip holds it.
    pub fn effect_owner(&self, slot: EffectSlotId) -> Option<MixerNode> {
        let project = self.project();
        project
            .tracks
            .iter()
            .map(|track| (Some(track.id), &track.mixer))
            .chain(std::iter::once((None, &project.master)))
            .find(|(_, strip)| strip.effects.iter().any(|effect| effect.id == slot))
            .map(|(owner, _)| MixerNode(owner))
    }
}

/// The destination a strip node stands for as a routing target: `None` is the master.
pub fn destination(owner: Option<TrackId>) -> Destination {
    owner.map_or(Destination::Master, Destination::Bus)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::Output;

    #[test]
    fn extra_outputs_participate_in_cycle_validation_and_invalid_edits_keep_redo() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let a = session.add_bus_track("A");
        let b = session.add_bus_track("B");
        let c = session.add_bus_track("C");
        session.add_track_output(a, Destination::Bus(b)).unwrap();
        session.add_track_output(a, Destination::Bus(c)).unwrap();
        let before = session.mixer_graph();
        for bus in [b, c] {
            assert!(session.add_track_output(bus, Destination::Bus(a)).is_err());
        }
        assert_eq!(session.mixer_graph(), before);
        session.undo();
        assert_eq!(
            session.project().track(a).unwrap().outputs,
            vec![Output::master(), Output::to_bus(b)]
        );
        assert!(
            session
                .set_track_outputs(a, &[Output::master(), Output::to_bus(TrackId(9999))])
                .is_err()
        );
        session.redo();
        assert_eq!(session.mixer_graph(), before);
    }

    #[test]
    fn deleting_mixed_branches_is_atomic_and_undo_restores_output_automation() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let a = session.add_bus_track("A");
        let b = session.add_bus_track("B");
        let c = session.add_bus_track("C");
        session.add_track_output(a, Destination::Bus(b)).unwrap();
        session.add_track_output(a, Destination::Bus(c)).unwrap();
        let slot = session.add_effect(Some(c), "auris.fx.compressor").unwrap();
        session
            .set_effect_sidechain(Some(c), slot, Some(a))
            .unwrap();
        session.set_automation_point(
            crate::prelude::ParamTarget::Output {
                track: a,
                to: Destination::Bus(c),
                tap: Tap::PostFader,
            },
            crate::prelude::Ticks::ZERO,
            -6.0,
        );
        let edges: Vec<_> = session
            .mixer_graph()
            .edges
            .into_iter()
            .filter(|edge| edge.source == MixerNode(Some(a)))
            .collect();
        let before = serde_json::to_value(session.project()).unwrap();
        assert!(session.disconnect_mixer_edges(&edges).unwrap());
        assert!(session.project().track(a).unwrap().outputs.is_empty());
        assert_eq!(session.effect_sidechain(Some(c), slot), None);
        assert!(!session.disconnect_mixer_edges(&edges).unwrap());
        session.undo();
        assert_eq!(serde_json::to_value(session.project()).unwrap(), before);
        session.redo();
        assert!(session.project().track(a).unwrap().outputs.is_empty());
    }

    #[test]
    fn graph_connections_reject_feedback_through_outputs_and_sidechains() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let a = session.add_bus_track("A");
        let b = session.add_bus_track("B");
        let c = session.add_bus_track("C");
        assert!(
            session
                .connect_mixer(a, MixerPort::Input(MixerNode(Some(b))))
                .unwrap()
        );
        assert!(
            session
                .connect_mixer(b, MixerPort::Input(MixerNode(Some(c))))
                .unwrap()
        );
        let before = session.mixer_graph();
        assert!(
            !session
                .connect_mixer(c, MixerPort::Input(MixerNode(Some(a))))
                .unwrap()
        );
        assert!(
            !session
                .connect_mixer(a, MixerPort::Input(MixerNode(Some(a))))
                .unwrap()
        );
        assert_eq!(session.mixer_graph(), before);
        session.undo();
        assert_eq!(
            session.project().track(b).unwrap().outputs,
            vec![Output::master()]
        );
        session.redo();
        assert_eq!(session.mixer_graph(), before);
    }

    #[test]
    fn every_strip_is_one_node_however_many_effects_it_carries() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_audio_track("Audio");
        session.add_effect(None, "auris.fx.compressor").unwrap();
        session.add_effect(None, "auris.fx.reverb").unwrap();
        session.add_effect(Some(track), "auris.fx.reverb").unwrap();
        let graph = session.mixer_graph();
        assert_eq!(graph.nodes, vec![MixerNode(Some(track)), MixerNode::MASTER]);
        assert_eq!(
            graph.edges,
            vec![MixerEdge {
                source: MixerNode(Some(track)),
                target: MixerNode::MASTER,
                kind: MixerEdgeKind::Output {
                    track,
                    gain_db: 0.0,
                    tap: Tap::PostFader,
                },
            }]
        );
    }

    #[test]
    fn graph_keeps_output_taps_after_inserts_with_explicit_fader_modes() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_bus_track("Source");
        let bus = session.add_bus_track("Return");
        let to = Destination::Bus(bus);
        session.add_effect(Some(track), "auris.fx.reverb").unwrap();
        session.add_track_output(track, to).unwrap();
        session
            .set_output_tap(track, to, Tap::PostFader, Tap::PreFader)
            .unwrap();
        let graph = session.mixer_graph();
        let edge = graph
            .edges
            .iter()
            .find(|e| e.target == MixerNode(Some(bus)))
            .unwrap();
        assert_eq!(edge.source, MixerNode(Some(track)));
        assert!(matches!(
            edge.kind,
            MixerEdgeKind::Output {
                tap: Tap::PreFader,
                ..
            }
        ));
        session
            .set_output_tap(track, to, Tap::PreFader, Tap::PostFader)
            .unwrap();
        assert!(
            session
                .mixer_graph()
                .edges
                .iter()
                .any(|e| e.source == MixerNode(Some(track))
                    && matches!(
                        e.kind,
                        MixerEdgeKind::Output {
                            tap: Tap::PostFader,
                            ..
                        }
                    ))
        );
    }

    #[test]
    fn graph_sidechains_participate_in_cycle_detection_and_undo() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let source = session.add_bus_track("Source");
        let listener = session.add_bus_track("Listener");
        let slot = session
            .add_effect(Some(listener), "auris.fx.compressor")
            .unwrap();
        let target = MixerPort::Key(MixerNode(Some(listener)), slot);
        let before = session.mixer_graph();
        assert!(session.connect_mixer(source, target).unwrap());
        assert!(
            !session
                .connect_mixer(listener, MixerPort::Input(MixerNode(Some(source))))
                .unwrap()
        );
        assert!(!session.connect_mixer(listener, target).unwrap());
        assert!(
            session
                .mixer_graph()
                .edges
                .iter()
                .any(|edge| edge.source == MixerNode(Some(source))
                    && edge.target == MixerNode(Some(listener))
                    && edge.kind == MixerEdgeKind::Sidechain { slot })
        );
        session.undo();
        assert_eq!(session.mixer_graph(), before);
    }

    #[test]
    fn graph_refuses_stale_nodes_non_bus_inputs_and_duplicate_outputs() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_audio_track("Audio");
        let bus = session.add_bus_track("Bus");
        assert!(
            !session
                .connect_mixer(bus, MixerPort::Input(MixerNode(Some(track))))
                .unwrap()
        );
        assert!(
            !session
                .connect_mixer(track, MixerPort::Input(MixerNode(Some(TrackId(u64::MAX)))))
                .unwrap()
        );
        assert!(
            !session
                .connect_mixer(
                    track,
                    MixerPort::Key(MixerNode::MASTER, EffectSlotId(u64::MAX))
                )
                .unwrap()
        );
        assert!(
            session
                .connect_mixer(track, MixerPort::Input(MixerNode(Some(bus))))
                .unwrap()
        );
        assert!(
            !session
                .connect_mixer(track, MixerPort::Input(MixerNode(Some(bus))))
                .unwrap()
        );
        assert_eq!(session.project().track(track).unwrap().outputs.len(), 2);
    }

    #[test]
    fn graph_routing_and_insert_order_survive_save_and_open() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let source = session.add_audio_track("Source");
        let bus = session.add_bus_track("Return");
        let first = session
            .add_effect(Some(bus), "auris.fx.compressor")
            .unwrap();
        session.add_effect(Some(bus), "auris.fx.reverb").unwrap();
        session.move_effect(Some(bus), first, 1);
        session
            .connect_mixer(source, MixerPort::Input(MixerNode(Some(bus))))
            .unwrap();
        let master_slot = session.add_effect(None, "auris.fx.compressor").unwrap();
        session
            .set_effect_sidechain(None, master_slot, Some(source))
            .unwrap();
        let graph = session.mixer_graph();
        let saved = session
            .save_as(&directory.path().join("Graph.auris"))
            .unwrap();
        let mut reopened = Session::new(SessionOptions::headless()).unwrap();
        reopened.open(&saved.document).unwrap();
        assert_eq!(reopened.mixer_graph(), graph);
    }
}
