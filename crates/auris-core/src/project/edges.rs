//! Every connection between tracks, in one list, and the nodes that only exist between them.
//!
//! A track's audio reaching a bus, a track keying an effect's detector, a layer playing another
//! track's notes: each of these used to be written down on one end of the connection — the
//! outputs on the track they left, the key on the slot that listened, the source on the layer.
//! Three stores meant three sets of rules for adding, removing, copying and repairing them, and
//! three walks for every question about the graph as a whole. [`Routing`] is the one store, and
//! an [`Edge`] says which kind of connection it is.
//!
//! Most nodes are still the document's own: tracks, the master and effect slots. A note track is
//! two of them on the notes side — its **score**, the clips it holds, and its **instrument**, what
//! plays notes — joined by a notes edge like any other, so a score can feed several instruments
//! and an instrument can play several scores at once. The one kind of node that lives here is the
//! [`Performance`]: a stack of [`NoteTransform`]s standing on a notes path between a score and an
//! instrument. It is a node rather than a property of the connection so that one performance can
//! feed several instruments.
//!
//! What an edge connects is a pair of [`Port`]s, and every port carries one kind of [`Signal`],
//! so a connection between ports that carry different things can be refused by asking them rather
//! than by remembering which combinations make sense.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::routing::{Destination, Output, Tap};
use super::transform::NoteTransform;
use super::{EffectSlotId, PerformanceId, TrackId};

/// What travels along a connection.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Signal {
    /// Notes, resolved before playback rather than on the audio thread.
    Notes,
    /// Sample frames.
    Audio,
}

/// A node notes can leave or arrive at.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotesNode {
    /// A note track's clips, as their own performance stacks play them. Notes only leave it.
    Score(TrackId),
    /// A note track's instrument. Notes only arrive at it; what arrives on every edge is merged.
    Instrument(TrackId),
    /// A performance standing on the path.
    Performance(PerformanceId),
}

impl NotesNode {
    /// The track this node belongs to, when it is a score or an instrument.
    pub fn track(self) -> Option<TrackId> {
        match self {
            NotesNode::Score(track) | NotesNode::Instrument(track) => Some(track),
            NotesNode::Performance(_) => None,
        }
    }
}

/// One end of a connection: a place a signal leaves or arrives.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Port {
    /// Notes leaving a score or a performance.
    NotesOut(NotesNode),
    /// Notes arriving at an instrument or a performance.
    NotesIn(NotesNode),
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

/// A stack of note transforms standing on a notes path.
///
/// What arrives is the source's notes as its clips perform them, clip by clip; what leaves is
/// those notes through every transform here in order. The clip's own stack has already run, so a
/// performance is a second stage over the whole part — a delay, a key split, a top voice, a swing
/// for every clip at once — and freezing a layer writes both stages in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Performance {
    /// Unique within the project, from the same counter as every other id.
    pub id: PerformanceId,
    /// What the node is called on screen; empty for one nobody has named.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The stages, applied in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<NoteTransform>,
}

impl Performance {
    /// An empty, unnamed performance: the identity.
    pub fn new(id: PerformanceId) -> Self {
        Self {
            id,
            name: String::new(),
            transforms: Vec::new(),
        }
    }
}

/// One connection.
///
/// Each variant is one kind of connection rather than a pair of ports with optional extras, so a
/// level can only sit on audio into a mix.
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
    /// Notes from a score or a performance into an instrument or a performance.
    Notes {
        /// Where the notes come from.
        from: NotesNode,
        /// Where they go.
        to: NotesNode,
    },
}

impl Edge {
    /// An output from `from`.
    pub fn output(from: TrackId, output: Output) -> Self {
        Edge::Output { from, output }
    }

    /// Notes from `from` to `to`.
    pub fn notes(from: NotesNode, to: NotesNode) -> Self {
        Edge::Notes { from, to }
    }

    /// A note track's own score into its own instrument: what every note track starts with.
    pub fn own_notes(track: TrackId) -> Self {
        Edge::notes(NotesNode::Score(track), NotesNode::Instrument(track))
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

    /// The track this connection leaves, when it leaves a track.
    pub fn from_track(&self) -> Option<TrackId> {
        match self {
            Edge::Output { from, .. } | Edge::Key { from, .. } => Some(*from),
            Edge::Notes { from, .. } => from.track(),
        }
    }

    /// The track this connection arrives at, when it arrives at a track.
    ///
    /// A bus for an output into one, the instrument's track for notes; `None` for the master,
    /// for a key, whose effect slot is owned by a strip this enum does not know, and for a
    /// performance.
    pub fn to_track(&self) -> Option<TrackId> {
        match self {
            Edge::Output { output, .. } => output.bus(),
            Edge::Notes { to, .. } => to.track(),
            Edge::Key { .. } => None,
        }
    }

    /// `true` when this connection touches `track` at either end.
    pub fn touches(&self, track: TrackId) -> bool {
        self.from_track() == Some(track) || self.to_track() == Some(track)
    }

    /// `true` when this connection touches performance `id` at either end.
    pub fn touches_performance(&self, id: PerformanceId) -> bool {
        matches!(
            self,
            Edge::Notes { from, to }
                if *from == NotesNode::Performance(id) || *to == NotesNode::Performance(id)
        )
    }
}

/// One way notes reach an instrument: the score they leave and the performances on the way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotesPath {
    /// The track whose score is played.
    pub source: TrackId,
    /// The performances between the score and the instrument, nearest the score first.
    pub performances: Vec<PerformanceId>,
}

impl NotesPath {
    /// The nodes along the path, score first and `instrument` last.
    pub fn nodes(&self, instrument: TrackId) -> Vec<NotesNode> {
        std::iter::once(NotesNode::Score(self.source))
            .chain(
                self.performances
                    .iter()
                    .map(|id| NotesNode::Performance(*id)),
            )
            .chain(std::iter::once(NotesNode::Instrument(instrument)))
            .collect()
    }

    /// `true` when the path into `instrument` runs along the edge from `from` to `to`.
    pub fn runs_along(&self, instrument: TrackId, from: NotesNode, to: NotesNode) -> bool {
        self.nodes(instrument)
            .windows(2)
            .any(|pair| pair[0] == from && pair[1] == to)
    }
}

/// Every connection in a project, and the performances standing on notes paths.
///
/// The edges are a list rather than per-node maps, because the order is meaningful: a track's
/// outputs keep the order they were added in, and that order is how the engine and the meters
/// address them.
///
/// The invariants the session upholds, and `Project::repair_routing` restores on load: no two
/// outputs of one track share a destination and tap; an effect slot has at most one key; no two
/// notes edges join the same pair; a performance has exactly one notes input and lies on a path
/// from a score to an instrument.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Routing {
    /// The performances, in the order they were made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    performances: Vec<Performance>,
    /// Every connection, in order.
    #[serde(default)]
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

    /// Replaces every connection, keeping the performances.
    pub fn set_edges(&mut self, edges: Vec<Edge>) {
        self.edges = edges;
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

    /// Every performance, in the order they were made.
    pub fn performances(&self) -> &[Performance] {
        &self.performances
    }

    /// One performance.
    pub fn performance(&self, id: PerformanceId) -> Option<&Performance> {
        self.performances
            .iter()
            .find(|performance| performance.id == id)
    }

    /// One performance, to be changed in place.
    pub fn performance_mut(&mut self, id: PerformanceId) -> Option<&mut Performance> {
        self.performances
            .iter_mut()
            .find(|performance| performance.id == id)
    }

    /// Adds a performance standing on no path yet.
    pub fn add_performance(&mut self, performance: Performance) {
        self.performances.push(performance);
    }

    /// What feeds notes into `node` first, if anything. A performance has one input; an
    /// instrument may have several, and [`Routing::notes_inputs`] lists them all.
    pub fn notes_input(&self, node: NotesNode) -> Option<NotesNode> {
        self.notes_inputs(node).next()
    }

    /// Everything feeding notes into `node`, in the order the connections were made.
    pub fn notes_inputs(&self, node: NotesNode) -> impl Iterator<Item = NotesNode> + '_ {
        self.edges.iter().filter_map(move |edge| match edge {
            Edge::Notes { from, to } if *to == node => Some(*from),
            _ => None,
        })
    }

    /// `true` when notes go straight from `from` to `to`.
    pub fn has_notes_edge(&self, from: NotesNode, to: NotesNode) -> bool {
        self.edges
            .iter()
            .any(|edge| matches!(edge, Edge::Notes { from: f, to: t } if *f == from && *t == to))
    }

    /// The path back from `at` to the score it starts at, or `None` when it loops or breaks.
    pub fn path_from(&self, mut at: NotesNode) -> Option<NotesPath> {
        let mut performances = Vec::new();
        let mut seen = HashSet::new();
        loop {
            match at {
                NotesNode::Score(source) => {
                    performances.reverse();
                    return Some(NotesPath {
                        source,
                        performances,
                    });
                }
                NotesNode::Performance(id) => {
                    if !seen.insert(id) {
                        return None;
                    }
                    performances.push(id);
                    at = self.notes_input(at)?;
                }
                NotesNode::Instrument(_) => return None,
            }
        }
    }

    /// Every path into `track`'s instrument, in the order its connections were made. Paths that
    /// loop or break are left out.
    pub fn paths_into(&self, track: TrackId) -> Vec<NotesPath> {
        self.notes_inputs(NotesNode::Instrument(track))
            .filter_map(|from| self.path_from(from))
            .collect()
    }

    /// The first path into layer `track` from another track's score, or `None` when `track` is
    /// no layer.
    pub fn path_into(&self, track: TrackId) -> Option<NotesPath> {
        if !self.is_layer(track) {
            return None;
        }
        self.paths_into(track).into_iter().next()
    }

    /// The track layer `track` plays the notes of, the first when there are several.
    pub fn source_of(&self, track: TrackId) -> Option<TrackId> {
        self.path_into(track).map(|path| path.source)
    }

    /// `true` when `track` is a layer: its instrument plays other tracks' scores and not its own.
    pub fn is_layer(&self, track: TrackId) -> bool {
        let paths = self.paths_into(track);
        !paths.is_empty() && paths.iter().all(|path| path.source != track)
    }

    /// The performance feeding layer `track`'s instrument directly on its path, if one does.
    pub fn performance_into(&self, track: TrackId) -> Option<PerformanceId> {
        self.path_into(track)?.performances.last().copied()
    }

    /// Makes `layer`'s instrument play `source`'s score directly, or its own again when `source`
    /// is `None`. Everything else that fed it goes, and performances that fed only it go with
    /// the connection.
    pub fn set_follows(&mut self, layer: TrackId, source: Option<TrackId>) {
        let target = NotesNode::Instrument(layer);
        self.edges
            .retain(|edge| !matches!(edge, Edge::Notes { to, .. } if *to == target));
        self.edges.push(Edge::notes(
            NotesNode::Score(source.unwrap_or(layer)),
            target,
        ));
        self.prune_performances();
    }

    /// Stands `performance` on the path into layer `layer`, directly in front of its instrument.
    ///
    /// Returns `false`, changing nothing, when `layer` is no layer.
    pub fn insert_performance(&mut self, layer: TrackId, performance: Performance) -> bool {
        let Some(path) = self.path_into(layer) else {
            return false;
        };
        let target = NotesNode::Instrument(layer);
        let last = path
            .performances
            .last()
            .map_or(NotesNode::Score(path.source), |id| {
                NotesNode::Performance(*id)
            });
        let node = NotesNode::Performance(performance.id);
        self.performances.push(performance);
        if let Some(edge) = self
            .edges
            .iter_mut()
            .find(|edge| matches!(edge, Edge::Notes { from, to } if *from == last && *to == target))
        {
            *edge = Edge::notes(last, node);
        }
        self.edges.push(Edge::notes(node, target));
        true
    }

    /// Takes performance `id` off its path, joining what fed it straight to everything it fed.
    /// Returns `false`, changing nothing, when there is no such performance.
    pub fn bypass_performance(&mut self, id: PerformanceId) -> bool {
        let node = NotesNode::Performance(id);
        if self.performance(id).is_none() {
            return false;
        }
        let input = self.notes_input(node);
        self.edges
            .retain(|edge| !matches!(edge, Edge::Notes { to, .. } if *to == node));
        match input {
            Some(input) => {
                for edge in &mut self.edges {
                    if let Edge::Notes { from, .. } = edge
                        && *from == node
                    {
                        *from = input;
                    }
                }
                self.dedup_notes();
            }
            None => {
                self.edges
                    .retain(|edge| !matches!(edge, Edge::Notes { from, .. } if *from == node));
            }
        }
        self.performances.retain(|performance| performance.id != id);
        true
    }

    /// Drops every notes edge that repeats an earlier one.
    pub fn dedup_notes(&mut self) -> bool {
        let mut seen = HashSet::new();
        self.retain(|edge| match edge {
            Edge::Notes { from, to } => seen.insert((*from, *to)),
            _ => true,
        })
    }

    /// The layers whose notes come from `source`, in the order their connections were made.
    pub fn layers_of(&self, source: TrackId) -> Vec<TrackId> {
        let mut layers = Vec::new();
        for edge in &self.edges {
            if let Edge::Notes {
                to: NotesNode::Instrument(layer),
                ..
            } = edge
                && !layers.contains(layer)
                && self.is_layer(*layer)
                && self
                    .paths_into(*layer)
                    .iter()
                    .any(|path| path.source == source)
            {
                layers.push(*layer);
            }
        }
        layers
    }

    /// Removes every performance that no longer lies on a path from a score to an instrument,
    /// with its connections. Returns `true` when any went.
    ///
    /// A performance nothing feeds has nothing to perform, and one that feeds nothing is heard by
    /// nobody; both are what is left when a layer is detached or deleted.
    pub fn prune_performances(&mut self) -> bool {
        let mut removed = false;
        loop {
            let dead: Vec<PerformanceId> = self
                .performances
                .iter()
                .map(|performance| performance.id)
                .filter(|&id| {
                    let node = NotesNode::Performance(id);
                    let fed = self.notes_input(node).is_some();
                    let feeds = self
                        .edges
                        .iter()
                        .any(|edge| matches!(edge, Edge::Notes { from, .. } if *from == node));
                    !fed || !feeds
                })
                .collect();
            if dead.is_empty() {
                return removed;
            }
            removed = true;
            self.performances
                .retain(|performance| !dead.contains(&performance.id));
            self.edges
                .retain(|edge| !dead.iter().any(|&id| edge.touches_performance(id)));
        }
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
            Edge::notes(
                NotesNode::Score(TrackId(1)),
                NotesNode::Instrument(TrackId(3)),
            ),
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
        routing.push(Edge::output(TrackId(1), Output::master()));
        routing.push(Edge::output(TrackId(2), Output::master()));
        routing.set_outputs(TrackId(1), &[Output::to_bus(TrackId(5)), Output::master()]);
        let order: Vec<_> = routing
            .edges()
            .iter()
            .filter_map(Edge::from_track)
            .collect();
        assert_eq!(order, [TrackId(1), TrackId(1), TrackId(2)]);
        assert_eq!(
            routing.outputs_of(TrackId(1)),
            [Output::to_bus(TrackId(5)), Output::master()]
        );
        routing.set_outputs(TrackId(1), &[]);
        assert_eq!(routing.edges().len(), 1);
    }

    #[test]
    fn a_layer_reads_its_source_through_every_performance_on_the_way() {
        let mut routing = Routing::new();
        let (source, layer) = (TrackId(1), TrackId(2));
        routing.push(Edge::own_notes(source));
        routing.set_follows(layer, Some(source));
        assert_eq!(routing.source_of(layer), Some(source));
        assert!(routing.insert_performance(layer, Performance::new(PerformanceId(7))));
        assert!(routing.insert_performance(layer, Performance::new(PerformanceId(8))));
        assert_eq!(
            routing.path_into(layer),
            Some(NotesPath {
                source,
                performances: vec![PerformanceId(7), PerformanceId(8)],
            })
        );
        assert_eq!(routing.performance_into(layer), Some(PerformanceId(8)));
        assert_eq!(routing.layers_of(source), [layer]);

        // Detaching the layer leaves both performances feeding nobody, so they go too, and the
        // layer plays its own score again.
        routing.set_follows(layer, None);
        assert!(routing.performances().is_empty());
        assert_eq!(
            routing.edges(),
            [Edge::own_notes(source), Edge::own_notes(layer)]
        );
        assert!(!routing.is_layer(layer));
    }

    #[test]
    fn bypassing_a_performance_joins_what_fed_it_to_what_it_fed() {
        let mut routing = Routing::new();
        routing.set_follows(TrackId(2), Some(TrackId(1)));
        routing.insert_performance(TrackId(2), Performance::new(PerformanceId(5)));
        routing.insert_performance(TrackId(2), Performance::new(PerformanceId(6)));
        assert!(routing.bypass_performance(PerformanceId(5)));
        assert_eq!(
            routing.path_into(TrackId(2)).unwrap().performances,
            [PerformanceId(6)]
        );
        assert!(!routing.bypass_performance(PerformanceId(5)));
    }

    #[test]
    fn a_performance_may_feed_several_layers() {
        let mut routing = Routing::new();
        let source = TrackId(1);
        routing.set_follows(TrackId(2), Some(source));
        routing.insert_performance(TrackId(2), Performance::new(PerformanceId(9)));
        routing.push(Edge::notes(
            NotesNode::Performance(PerformanceId(9)),
            NotesNode::Instrument(TrackId(3)),
        ));
        assert_eq!(routing.layers_of(source), [TrackId(2), TrackId(3)]);
        routing.set_follows(TrackId(2), None);
        assert_eq!(routing.source_of(TrackId(3)), Some(source), "still fed");
        assert_eq!(routing.performances().len(), 1);
    }

    #[test]
    fn an_instrument_playing_its_own_score_and_another_is_no_layer() {
        let mut routing = Routing::new();
        let (a, b) = (TrackId(1), TrackId(2));
        routing.push(Edge::own_notes(a));
        routing.push(Edge::own_notes(b));
        routing.push(Edge::notes(NotesNode::Score(a), NotesNode::Instrument(b)));
        assert_eq!(routing.paths_into(b).len(), 2);
        assert!(!routing.is_layer(b), "it still plays a score of its own");
        assert!(routing.layers_of(a).is_empty());

        // Cut its own score and it is a layer of `a`.
        routing.retain(|edge| *edge != Edge::own_notes(b));
        assert!(routing.is_layer(b));
        assert_eq!(routing.source_of(b), Some(a));
        assert_eq!(routing.layers_of(a), [b]);

        // Cut everything and it is neither: a track whose score feeds nothing plays nothing.
        routing.retain(|edge| edge.to_track() != Some(b));
        assert!(!routing.is_layer(b));
        assert!(routing.paths_into(b).is_empty());
    }

    #[test]
    fn a_path_knows_the_edges_it_runs_along() {
        let path = NotesPath {
            source: TrackId(1),
            performances: vec![PerformanceId(4)],
        };
        let p = NotesNode::Performance(PerformanceId(4));
        assert!(path.runs_along(TrackId(2), NotesNode::Score(TrackId(1)), p));
        assert!(path.runs_along(TrackId(2), p, NotesNode::Instrument(TrackId(2))));
        assert!(!path.runs_along(
            TrackId(2),
            NotesNode::Score(TrackId(1)),
            NotesNode::Instrument(TrackId(2))
        ));
    }

    #[test]
    fn routing_round_trips_through_json_tagged_by_kind() {
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
        routing.set_follows(TrackId(3), Some(TrackId(1)));
        let mut performance = Performance::new(PerformanceId(5));
        performance
            .transforms
            .push(NoteTransform::Delay { ticks: 240 });
        routing.insert_performance(TrackId(3), performance);
        let json = serde_json::to_value(&routing).unwrap();
        assert_eq!(json["edges"][0]["kind"], "output");
        assert_eq!(json["edges"][0]["to"]["bus"], 2);
        assert_eq!(json["edges"][1]["kind"], "key");
        assert_eq!(json["edges"][2]["kind"], "notes");
        assert_eq!(json["edges"][2]["from"]["score"], 1);
        assert_eq!(json["performances"][0]["transforms"][0]["kind"], "delay");
        assert_eq!(serde_json::from_value::<Routing>(json).unwrap(), routing);
    }
}
