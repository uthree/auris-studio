//! The audible mixer as a directed acyclic graph, shared by every frontend.
//!
//! Nodes are stable document identities, not plugin instances. Internal edges describe the
//! insert order; editable edges use the same session commands as the console and the toolbox.

use auris_core::{Destination, EffectSlotId, Tap, TrackId};

use crate::{Session, SessionError};

pub mod commands;

/// A strip or insert in the signal path. `None` identifies the master strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MixerNode {
    /// A track or summing bus with its gain, pan, mute and solo controls. Inserts precede
    /// the fader even though its controls share this node with the source.
    Strip(Option<TrackId>),
    /// An insert before its strip's fader.
    Effect(Option<TrackId>, EffectSlotId),
}

/// The operation represented by an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MixerEdgeKind {
    /// Consecutive points inside one strip.
    Chain,
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
    /// A key signal arriving at an effect's second input.
    Sidechain,
    /// A layer playing its source's notes: from the source's strip to the layer's.
    ///
    /// Notes rather than audio, so it carries no level and no meter; disconnecting it detaches
    /// the layer.
    Notes,
}

/// One directed signal connection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerEdge {
    /// Where the audio leaves.
    pub source: MixerNode,
    /// Where the audio arrives.
    pub target: MixerNode,
    /// The role of this connection.
    pub kind: MixerEdgeKind,
}

/// A document projection; reading it does not build plugins or touch the audio thread.
#[derive(Clone, Debug, PartialEq)]
pub struct MixerGraph {
    /// Track strips in document order, followed by the master strip.
    pub nodes: Vec<MixerNode>,
    /// All insert, output and sidechain connections.
    pub edges: Vec<MixerEdge>,
}

impl Session {
    /// Disconnect an exact set of editable graph edges as one undo step.
    /// Stale edges or insert-chain edges reject the complete request before history changes.
    pub fn disconnect_mixer_edges(&mut self, edges: &[MixerEdge]) -> Result<bool, SessionError> {
        let graph = self.mixer_graph();
        if edges.is_empty()
            || edges.iter().any(|edge| {
                matches!(edge.kind, MixerEdgeKind::Chain) || !graph.edges.contains(edge)
            })
        {
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
                        if let MixerNode::Strip(target) = edge.target {
                            self.remove_track_output(track, destination(target), tap)?;
                        }
                    }
                    MixerEdgeKind::Sidechain => {
                        if let MixerNode::Effect(owner, slot) = edge.target {
                            self.set_effect_sidechain(owner, slot, None)?;
                        }
                    }
                    MixerEdgeKind::Notes => {
                        if let MixerNode::Strip(Some(layer)) = edge.target {
                            self.detach_layer(layer)?;
                        }
                    }
                    MixerEdgeKind::Chain => unreachable!(),
                }
            }
            Ok(true)
        })();
        self.end_transaction();
        result
    }

    /// The terminal node of a strip's inserts, where its signal leaves.
    /// A pre-fader output leaves the same node, told apart by its [`Tap`].
    pub fn mixer_graph_output(&self, owner: Option<TrackId>) -> MixerNode {
        owner
            .and_then(|id| self.project().track(id))
            .map_or(&self.project().master, |track| &track.mixer)
            .effects
            .last()
            .map_or(MixerNode::Strip(owner), |slot| {
                MixerNode::Effect(owner, slot.id)
            })
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
            let input = MixerNode::Strip(owner);
            graph.nodes.push(input);
            let mut previous = input;
            for slot in &strip.effects {
                let node = MixerNode::Effect(owner, slot.id);
                graph.nodes.push(node);
                graph.edges.push(MixerEdge {
                    source: previous,
                    target: node,
                    kind: MixerEdgeKind::Chain,
                });
                if let Some(source) = self.project().sidechain(slot.id) {
                    graph.edges.push(MixerEdge {
                        source: self.mixer_graph_output(Some(source)),
                        target: node,
                        kind: MixerEdgeKind::Sidechain,
                    });
                }
                previous = node;
            }
            if let Some(track) = owner.and_then(|id| self.project().track(id)) {
                if let Some(source) = self.project().follows(track.id)
                    && self.project().track(source.track).is_some()
                {
                    graph.edges.push(MixerEdge {
                        source: MixerNode::Strip(Some(source.track)),
                        target: input,
                        kind: MixerEdgeKind::Notes,
                    });
                }
                for output in self.project().routing.outputs(track.id) {
                    graph.edges.push(MixerEdge {
                        source: previous,
                        target: MixerNode::Strip(output.bus()),
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

    /// Whether a graph gesture can connect a track's signal to this node.
    ///
    /// An input receives a new unity output after the fader. An effect receives a sidechain.
    /// Self connections, duplicates, stale identities and all cycles are rejected.
    pub fn can_connect_mixer(&mut self, source: TrackId, target: MixerNode) -> bool {
        if self.project().track(source).is_none() {
            return false;
        }
        match target {
            MixerNode::Strip(owner) => {
                if let Some(bus) = owner
                    && (!self.project().track(bus).is_some_and(|t| t.kind.is_bus())
                        || self.project().routing_would_cycle(source, bus))
                {
                    return false;
                }
                let key = (destination(owner), Tap::PostFader);
                !self
                    .project()
                    .routing
                    .outputs(source)
                    .any(|output| output.key() == key)
            }
            MixerNode::Effect(owner, slot) => {
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
        target: MixerNode,
    ) -> Result<bool, SessionError> {
        if !self.can_connect_mixer(source, target) {
            return Ok(false);
        }
        match target {
            MixerNode::Strip(owner) => self.add_track_output(source, destination(owner))?,
            MixerNode::Effect(owner, slot) => {
                self.set_effect_sidechain(owner, slot, Some(source))?
            }
        }
        Ok(true)
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
            session.project().outputs(a),
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
            .filter(|edge| match edge.source {
                MixerNode::Strip(owner) | MixerNode::Effect(owner, _) => owner == Some(a),
            })
            .collect();
        let before = serde_json::to_value(session.project()).unwrap();
        assert!(session.disconnect_mixer_edges(&edges).unwrap());
        assert!(session.project().outputs(a).is_empty());
        assert_eq!(session.effect_sidechain(Some(c), slot), None);
        assert!(!session.disconnect_mixer_edges(&edges).unwrap());
        session.undo();
        assert_eq!(serde_json::to_value(session.project()).unwrap(), before);
        session.redo();
        assert!(session.project().outputs(a).is_empty());
    }

    #[test]
    fn graph_connections_reject_feedback_through_outputs_and_sidechains() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let a = session.add_bus_track("A");
        let b = session.add_bus_track("B");
        let c = session.add_bus_track("C");
        assert!(session.connect_mixer(a, MixerNode::Strip(Some(b))).unwrap());
        assert!(session.connect_mixer(b, MixerNode::Strip(Some(c))).unwrap());
        let before = session.mixer_graph();
        assert!(!session.connect_mixer(c, MixerNode::Strip(Some(a))).unwrap());
        assert!(!session.connect_mixer(a, MixerNode::Strip(Some(a))).unwrap());
        assert_eq!(session.mixer_graph(), before);
        session.undo();
        assert_eq!(session.project().outputs(b), vec![Output::master()]);
        session.redo();
        assert_eq!(session.mixer_graph(), before);
    }

    #[test]
    fn graph_keeps_output_taps_after_inserts_with_explicit_fader_modes() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_bus_track("Source");
        let bus = session.add_bus_track("Return");
        let to = Destination::Bus(bus);
        let slot = session.add_effect(Some(track), "auris.fx.reverb").unwrap();
        session.add_track_output(track, to).unwrap();
        session
            .set_output_tap(track, to, Tap::PostFader, Tap::PreFader)
            .unwrap();
        let graph = session.mixer_graph();
        let edge = graph
            .edges
            .iter()
            .find(|e| e.target == MixerNode::Strip(Some(bus)))
            .unwrap();
        assert_eq!(edge.source, MixerNode::Effect(Some(track), slot));
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
        assert!(session.mixer_graph().edges.iter().any(|e| e.source
            == MixerNode::Effect(Some(track), slot)
            && matches!(
                e.kind,
                MixerEdgeKind::Output {
                    tap: Tap::PostFader,
                    ..
                }
            )));
    }

    #[test]
    fn graph_sidechains_participate_in_cycle_detection_and_undo() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let source = session.add_bus_track("Source");
        let listener = session.add_bus_track("Listener");
        let slot = session
            .add_effect(Some(listener), "auris.fx.compressor")
            .unwrap();
        let target = MixerNode::Effect(Some(listener), slot);
        let before = session.mixer_graph();
        assert!(session.connect_mixer(source, target).unwrap());
        assert!(
            !session
                .connect_mixer(listener, MixerNode::Strip(Some(source)))
                .unwrap()
        );
        assert!(!session.connect_mixer(listener, target).unwrap());
        assert!(
            session
                .mixer_graph()
                .edges
                .iter()
                .any(|edge| edge.source == MixerNode::Strip(Some(source))
                    && edge.target == target
                    && edge.kind == MixerEdgeKind::Sidechain)
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
                .connect_mixer(bus, MixerNode::Strip(Some(track)))
                .unwrap()
        );
        assert!(
            !session
                .connect_mixer(track, MixerNode::Strip(Some(TrackId(u64::MAX))))
                .unwrap()
        );
        assert!(
            !session
                .connect_mixer(track, MixerNode::Effect(None, EffectSlotId(u64::MAX)))
                .unwrap()
        );
        assert!(
            session
                .connect_mixer(track, MixerNode::Strip(Some(bus)))
                .unwrap()
        );
        assert!(
            !session
                .connect_mixer(track, MixerNode::Strip(Some(bus)))
                .unwrap()
        );
        assert_eq!(session.project().outputs(track).len(), 2);
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
            .connect_mixer(source, MixerNode::Strip(Some(bus)))
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
