//! Every connection between tracks, in one list.
//!
//! A track's audio reaching a bus, a track keying an effect's detector, a layer playing another
//! track's notes: each of these used to be written down on one end of the connection — the
//! outputs on the track they left, the key on the slot that listened, the source on the layer.
//! Three stores meant three sets of rules for adding, removing, copying and repairing them, and
//! three walks for every question about the graph as a whole. [`Routing`] is the one store, and
//! an [`Edge`] says which kind of connection it is.
//!
//! The nodes are still the document's own: tracks, the master and effect slots. What an edge
//! connects is a pair of [`Port`]s, and every port carries one kind of [`Signal`], so a
//! connection between ports that carry different things can be refused by asking them rather
//! than by remembering which combinations make sense.

use serde::{Deserialize, Serialize};

use super::layer::{LayerShape, NoteSource};
use super::routing::{Destination, Output, Tap};
use super::{EffectSlotId, TrackId};

/// What travels along a connection.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Signal {
    /// Notes, resolved before playback rather than on the audio thread.
    Notes,
    /// Sample frames.
    Audio,
}

/// One end of a connection: a place a signal leaves or arrives.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Port {
    /// A track's notes, as its clips perform them.
    NotesOut(TrackId),
    /// The notes a layer plays instead of its own.
    NotesIn(TrackId),
    /// A track's audio, taken after the fader or before it.
    AudioOut(TrackId, Tap),
    /// The summing input of a bus or the master.
    Mix(Destination),
    /// An effect's detector input.
    Key(EffectSlotId),
}

impl Port {
    /// What this port carries.
    pub fn signal(self) -> Signal {
        match self {
            Port::NotesOut(_) | Port::NotesIn(_) => Signal::Notes,
            Port::AudioOut(..) | Port::Mix(_) | Port::Key(_) => Signal::Audio,
        }
    }

    /// `true` for a port a signal leaves from.
    pub fn is_source(self) -> bool {
        matches!(self, Port::NotesOut(_) | Port::AudioOut(..))
    }
}

/// One connection.
///
/// Each variant is one kind of connection rather than a pair of ports with optional extras, so a
/// level can only sit on audio into a mix and a shape only on notes into a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Edge {
    /// A track's audio into a bus or the master, at a level and tap of its own.
    Output {
        /// The track the audio leaves.
        from: TrackId,
        /// Where it lands, how loud, and where on the strip it is taken.
        #[serde(flatten)]
        output: Output,
    },
    /// A track's audio into an effect's detector: a sidechain.
    ///
    /// Taken after the track's fader, pan and mute — what the track puts into the mix — so that
    /// turning a kick down ducks less and muting it stops the duck.
    Key {
        /// The track that keys.
        from: TrackId,
        /// The effect slot listening to it.
        to: EffectSlotId,
    },
    /// A track's notes into a layer, shaped on the way.
    Notes {
        /// The track whose notes are played. Never a layer itself.
        from: TrackId,
        /// The layer that plays them.
        to: TrackId,
        /// What is done to the notes.
        #[serde(default, skip_serializing_if = "LayerShape::is_identity")]
        shape: LayerShape,
    },
}

impl Edge {
    /// An output from `from`.
    pub fn output(from: TrackId, output: Output) -> Self {
        Edge::Output { from, output }
    }

    /// Where this connection leaves.
    pub fn source(&self) -> Port {
        match self {
            Edge::Output { from, output } => Port::AudioOut(*from, output.tap),
            Edge::Key { from, .. } => Port::AudioOut(*from, Tap::PostFader),
            Edge::Notes { from, .. } => Port::NotesOut(*from),
        }
    }

    /// Where this connection arrives.
    pub fn sink(&self) -> Port {
        match self {
            Edge::Output { output, .. } => Port::Mix(output.to),
            Edge::Key { to, .. } => Port::Key(*to),
            Edge::Notes { to, .. } => Port::NotesIn(*to),
        }
    }

    /// What this connection carries.
    pub fn signal(&self) -> Signal {
        self.source().signal()
    }

    /// The track this connection leaves.
    pub fn from(&self) -> TrackId {
        match self {
            Edge::Output { from, .. } | Edge::Key { from, .. } | Edge::Notes { from, .. } => *from,
        }
    }

    /// The track this connection arrives at, when it arrives at a track.
    ///
    /// A bus for an output into one, the layer for notes; `None` for the master and for a key,
    /// whose effect slot is owned by a strip this enum does not know.
    pub fn to_track(&self) -> Option<TrackId> {
        match self {
            Edge::Output { output, .. } => output.bus(),
            Edge::Notes { to, .. } => Some(*to),
            Edge::Key { .. } => None,
        }
    }

    /// `true` when this connection touches `track` at either end.
    pub fn touches(&self, track: TrackId) -> bool {
        self.from() == track || self.to_track() == Some(track)
    }
}

/// Every connection in a project.
///
/// A list rather than per-node maps, because the order is meaningful: a track's outputs keep the
/// order they were added in, and that order is how the engine and the meters address them.
///
/// The invariants the session upholds, and `Project::repair_routing` restores on load: no two
/// outputs of one track share a destination and tap; an effect slot has at most one key; a layer
/// follows at most one source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Routing {
    edges: Vec<Edge>,
}

impl Routing {
    /// An empty routing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every connection, in order.
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Adds a connection at the end.
    pub fn push(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    /// Keeps the connections `keep` approves, returning `true` when any went.
    pub fn retain(&mut self, mut keep: impl FnMut(&Edge) -> bool) -> bool {
        let before = self.edges.len();
        self.edges.retain(|edge| keep(edge));
        self.edges.len() != before
    }

    /// Every connection, to be changed in place.
    pub fn edges_mut(&mut self) -> impl Iterator<Item = &mut Edge> {
        self.edges.iter_mut()
    }

    /// The outputs of `track`, in order.
    pub fn outputs(&self, track: TrackId) -> impl Iterator<Item = &Output> {
        self.edges.iter().filter_map(move |edge| match edge {
            Edge::Output { from, output } if *from == track => Some(output),
            _ => None,
        })
    }

    /// The outputs of `track`, in order, collected.
    pub fn outputs_of(&self, track: TrackId) -> Vec<Output> {
        self.outputs(track).copied().collect()
    }

    /// One of `track`'s outputs, to be changed in place.
    pub fn output_mut(&mut self, track: TrackId, key: (Destination, Tap)) -> Option<&mut Output> {
        self.edges.iter_mut().find_map(|edge| match edge {
            Edge::Output { from, output } if *from == track && output.key() == key => Some(output),
            _ => None,
        })
    }

    /// Replaces every output of `track` with `outputs`, in that order.
    ///
    /// The new outputs take the place of the first old one, so a track's outputs stay where they
    /// were in the list; a track that had none gets them at the end.
    pub fn set_outputs(&mut self, track: TrackId, outputs: &[Output]) {
        let mine = |edge: &Edge| matches!(edge, Edge::Output { from, .. } if *from == track);
        let at = self.edges.iter().position(mine).unwrap_or(self.edges.len());
        self.edges.retain(|edge| !mine(edge));
        let at = at.min(self.edges.len());
        self.edges.splice(
            at..at,
            outputs.iter().map(|output| Edge::Output {
                from: track,
                output: *output,
            }),
        );
    }

    /// The buses `track` feeds, by any output.
    pub fn feeds(&self, track: TrackId) -> impl Iterator<Item = TrackId> + '_ {
        self.outputs(track).filter_map(|output| output.bus())
    }

    /// The track keying effect slot `slot`, if any.
    pub fn key(&self, slot: EffectSlotId) -> Option<TrackId> {
        self.edges.iter().find_map(|edge| match edge {
            Edge::Key { from, to } if *to == slot => Some(*from),
            _ => None,
        })
    }

    /// Keys `slot` from `source`, or clears its key when `source` is `None`.
    pub fn set_key(&mut self, slot: EffectSlotId, source: Option<TrackId>) {
        self.edges
            .retain(|edge| !matches!(edge, Edge::Key { to, .. } if *to == slot));
        if let Some(from) = source {
            self.edges.push(Edge::Key { from, to: slot });
        }
    }

    /// Every key connection, as (source track, effect slot).
    pub fn keys(&self) -> impl Iterator<Item = (TrackId, EffectSlotId)> + '_ {
        self.edges.iter().filter_map(|edge| match edge {
            Edge::Key { from, to } => Some((*from, *to)),
            _ => None,
        })
    }

    /// What `track` follows, when it is a layer.
    pub fn follows(&self, track: TrackId) -> Option<NoteSource> {
        self.edges.iter().find_map(|edge| match edge {
            Edge::Notes { from, to, shape } if *to == track => Some(NoteSource {
                track: *from,
                shape: shape.clone(),
            }),
            _ => None,
        })
    }

    /// The shape layer `track` plays its source through.
    pub fn shape(&self, track: TrackId) -> Option<&LayerShape> {
        self.edges.iter().find_map(|edge| match edge {
            Edge::Notes { to, shape, .. } if *to == track => Some(shape),
            _ => None,
        })
    }

    /// The shape layer `track` plays its source through, to be changed in place.
    pub fn shape_mut(&mut self, track: TrackId) -> Option<&mut LayerShape> {
        self.edges.iter_mut().find_map(|edge| match edge {
            Edge::Notes { to, shape, .. } if *to == track => Some(shape),
            _ => None,
        })
    }

    /// Makes `layer` follow `source`, or stop following anything when `source` is `None`.
    pub fn set_follows(&mut self, layer: TrackId, source: Option<NoteSource>) {
        self.edges
            .retain(|edge| !matches!(edge, Edge::Notes { to, .. } if *to == layer));
        if let Some(source) = source {
            self.edges.push(Edge::Notes {
                from: source.track,
                to: layer,
                shape: source.shape,
            });
        }
    }

    /// `true` when `track` is a layer.
    pub fn is_layer(&self, track: TrackId) -> bool {
        self.edges
            .iter()
            .any(|edge| matches!(edge, Edge::Notes { to, .. } if *to == track))
    }

    /// The layers following `source`, in the order they were made.
    pub fn layers_of(&self, source: TrackId) -> impl Iterator<Item = TrackId> + '_ {
        self.edges.iter().filter_map(move |edge| match edge {
            Edge::Notes { from, to, .. } if *from == source => Some(*to),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_edge_joins_ports_that_carry_the_same_signal() {
        let edges = [
            Edge::Output {
                from: TrackId(1),
                output: Output::to_bus(TrackId(2)),
            },
            Edge::Key {
                from: TrackId(1),
                to: EffectSlotId(9),
            },
            Edge::Notes {
                from: TrackId(1),
                to: TrackId(3),
                shape: LayerShape::default(),
            },
        ];
        for edge in &edges {
            assert!(edge.source().is_source());
            assert!(!edge.sink().is_source());
            assert_eq!(edge.source().signal(), edge.sink().signal());
        }
        assert_eq!(edges[2].signal(), Signal::Notes);
    }

    #[test]
    fn replacing_a_tracks_outputs_keeps_them_where_they_were() {
        let mut routing = Routing::new();
        routing.push(Edge::Output {
            from: TrackId(1),
            output: Output::master(),
        });
        routing.push(Edge::Output {
            from: TrackId(2),
            output: Output::master(),
        });
        routing.set_outputs(TrackId(1), &[Output::to_bus(TrackId(5)), Output::master()]);
        let order: Vec<_> = routing.edges().iter().map(Edge::from).collect();
        assert_eq!(order, [TrackId(1), TrackId(1), TrackId(2)]);
        assert_eq!(
            routing.outputs_of(TrackId(1)),
            [Output::to_bus(TrackId(5)), Output::master()]
        );
        routing.set_outputs(TrackId(1), &[]);
        assert_eq!(routing.edges().len(), 1);
    }

    #[test]
    fn edges_round_trip_through_json_tagged_by_kind() {
        let mut routing = Routing::new();
        routing.push(Edge::Output {
            from: TrackId(1),
            output: Output {
                to: Destination::Bus(TrackId(2)),
                gain_db: -6.0,
                tap: Tap::PreFader,
            },
        });
        routing.set_key(EffectSlotId(4), Some(TrackId(1)));
        routing.set_follows(TrackId(3), Some(NoteSource::new(TrackId(1))));
        let json = serde_json::to_value(&routing).unwrap();
        assert_eq!(json[0]["kind"], "output");
        assert_eq!(json[0]["to"]["bus"], 2);
        assert_eq!(json[1]["kind"], "key");
        assert_eq!(json[2]["kind"], "notes");
        assert_eq!(serde_json::from_value::<Routing>(json).unwrap(), routing);
    }
}
