//! The mixer strip, and where what leaves it goes.
//!
//! A [`MixerStrip`] is what a track sounds like — gain, pan, mute, solo and the effects in front
//! of the fader — and an [`Output`] with its [`AuxSend`]s is where that lands. The two are one
//! file because every question worth asking is about the pair: the order tracks have to be mixed
//! in, whether an edit would leave a bus waiting for itself, and which tracks a solo leaves
//! audible. All three are walks over [`Track::feeds`](super::Track::feeds), and not one of them
//! is about a clip.
//!
//! [`Project::repair_routing`] is here for the same reason: an edge that names something which
//! is not a bus, and an edge that closes a loop, are both faults in this graph and nothing else.

use serde::{Deserialize, Serialize};

use crate::asset::AssetPath;
use crate::plugin::PluginState;

use super::track::Track;
use super::{EffectSlotId, Project, SendId, TrackId};

/// One effect in a chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectSlot {
    /// Unique within the project.
    pub id: EffectSlotId,
    /// Registry id of the effect to instantiate.
    pub effect_id: String,
    /// Bypass switch. A bypassed effect is still instantiated so its state survives.
    pub enabled: bool,
    /// Saved parameter values.
    pub state: PluginState,
    /// The plugin file this slot's effect lives in, for an effect the registry cannot build.
    ///
    /// `None` for every built-in, which is what an id alone is enough to find. A hosted plugin
    /// needs the file as well, because its id was chosen by somebody else and says nothing about
    /// where it is.
    ///
    /// [`External`](AssetPath::External), never `Inside`: a plugin is a library shared by every
    /// project on the machine, exactly like a SoundFont, and copying Surge XT into a song folder
    /// would be absurd. The cost is that a project carried to another machine finds its plugins
    /// only if they are installed there — which is the same bargain every DAW makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<AssetPath>,
    /// The track whose signal keys this effect, for one that listens to a second input.
    ///
    /// A compressor keyed from the kick drum ducks the bass when the kick lands; a hosted plugin
    /// that declares a second audio input reads it there. `None` — which is every slot until
    /// somebody says otherwise — means the effect hears only the track it sits on.
    ///
    /// Named on the slot rather than on the source, because it is the *effect* that wants one:
    /// a track has no idea how many chains are keyed from it, and would not want to keep a list.
    /// That makes it the only edge in the routing that runs backwards from where it is written
    /// down, which is why [`Project::routing_order`] turns it round rather than reading it as it
    /// stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<TrackId>,
}

impl EffectSlot {
    /// An enabled slot with default parameters.
    pub fn new(id: EffectSlotId, effect_id: impl Into<String>) -> Self {
        Self {
            id,
            effect_id: effect_id.into(),
            enabled: true,
            state: PluginState::empty(),
            file: None,
            sidechain: None,
        }
    }

    /// A slot filled by a plugin hosted from a file.
    pub fn hosted(id: EffectSlotId, effect_id: impl Into<String>, file: AssetPath) -> Self {
        Self {
            file: Some(file),
            ..Self::new(id, effect_id)
        }
    }

    /// `true` when this slot names a plugin the registry cannot build.
    pub fn is_hosted(&self) -> bool {
        self.file.is_some()
    }
}

/// Where a track's output goes once its own strip has finished with it.
///
/// Two destinations rather than one field of [`Option<TrackId>`], because "no bus" is not an
/// absence: every track feeds *something*, and the master bus is a real place. A track whose
/// output is a bus is not routed to the master at all — the bus is, on its behalf.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Output {
    /// Straight to the master bus, which is where a track starts.
    #[default]
    Master,
    /// Into a bus track, mixed with everything else routed there.
    Bus(TrackId),
}

impl Output {
    /// The bus this names, or `None` for the master.
    pub fn bus(self) -> Option<TrackId> {
        match self {
            Output::Master => None,
            Output::Bus(id) => Some(id),
        }
    }
}

/// A copy of a track's signal, fed to a bus at a level of its own.
///
/// The difference between a send and an [`Output`] is that a send is a *tap*: the track carries on
/// to its own destination as well. That is what makes one reverb shared — six tracks send to it at
/// six different levels and all six are still heard dry.
///
/// A mixing desk calls this an *aux send*, and so does this type, for a reason that has nothing to
/// do with consoles: a document type called `Send` would shadow [`std::marker::Send`] in every file
/// that glob-imports the session's prelude, which is every file in both frontends. The error that
/// produces names a trait nobody mentioned, in a file that never touched the mixer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuxSend {
    /// Unique within the project.
    pub id: SendId,
    /// The bus this feeds.
    pub target: TrackId,
    /// How much of the signal is sent, in decibels.
    pub level_db: f32,
    /// Whether the copy is taken before the track's fader rather than after it.
    ///
    /// After it is the default and is what a reverb wants: pulling a track down should take its
    /// reverb with it. Before it is what a headphone mix wants, where the point is a balance that
    /// does not follow the one in the room.
    #[serde(default)]
    pub pre_fader: bool,
}

impl AuxSend {
    /// A post-fader send at unity.
    pub fn new(id: SendId, target: TrackId) -> Self {
        Self {
            id,
            target,
            level_db: 0.0,
            pre_fader: false,
        }
    }
}

/// Volume, pan, mute/solo and the effect chain for a track or the master bus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerStrip {
    /// Fader position in decibels.
    pub gain_db: f32,
    /// Stereo position, -1.0 (left) to 1.0 (right).
    pub pan: f32,
    /// Silences this strip.
    pub mute: bool,
    /// Silences every strip that is not soloed.
    pub solo: bool,
    /// Effects, applied in order before the fader.
    pub effects: Vec<EffectSlot>,
    /// How loud this strip is meant to be, in LUFS, or `None` for one nobody has aimed.
    ///
    /// A fader position is not a level: what a track is worth depends on the instrument that
    /// answered, and two instruments at the same number are not within ten decibels of each other.
    /// This is the level the fader is *trying* to reach, which is a thing a fader cannot say by
    /// itself — and it is what lets a mix be set by rendering it and measuring what came out.
    ///
    /// Written by the composer, which is the only part of this program that knows what a track is
    /// for. `None` everywhere else, and that is not a gap to be filled with a guess: a track
    /// somebody made by hand is aimed whereever they put it.
    #[serde(default)]
    pub target_lufs: Option<f32>,
}

impl MixerStrip {
    /// Every track keying an effect in this chain, one per slot that names one.
    ///
    /// Duplicates are left in: two effects keyed from the same drum are two edges as far as the
    /// routing is concerned, and the walk that reads them does not care how often it is told the
    /// same thing.
    pub fn sidechain_sources(&self) -> impl Iterator<Item = TrackId> + '_ {
        self.effects.iter().filter_map(|slot| slot.sidechain)
    }
}

impl Default for MixerStrip {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            effects: Vec::new(),
            target_lufs: None,
        }
    }
}

impl Project {
    /// Adds an effect to a track's chain, or to the master bus when `track_id` is `None`.
    pub fn add_effect(
        &mut self,
        track_id: Option<TrackId>,
        effect_id: impl Into<String>,
    ) -> Option<EffectSlotId> {
        let slot_id = EffectSlotId(self.allocate_id());
        let strip = match track_id {
            Some(id) => &mut self.track_mut(id)?.mixer,
            None => &mut self.master,
        };
        strip.effects.push(EffectSlot::new(slot_id, effect_id));
        Some(slot_id)
    }

    /// Adds an effect that has to be hosted from a file rather than built from the registry.
    pub fn add_hosted_effect(
        &mut self,
        track_id: Option<TrackId>,
        effect_id: impl Into<String>,
        file: AssetPath,
    ) -> Option<EffectSlotId> {
        let slot_id = EffectSlotId(self.allocate_id());
        let strip = match track_id {
            Some(id) => &mut self.track_mut(id)?.mixer,
            None => &mut self.master,
        };
        strip
            .effects
            .push(EffectSlot::hosted(slot_id, effect_id, file));
        Some(slot_id)
    }

    /// Removes an effect slot from anywhere in the project.
    pub fn remove_effect(&mut self, slot_id: EffectSlotId) -> bool {
        let mut removed = false;
        for strip in self
            .tracks
            .iter_mut()
            .map(|track| &mut track.mixer)
            .chain(std::iter::once(&mut self.master))
        {
            let before = strip.effects.len();
            strip.effects.retain(|slot| slot.id != slot_id);
            removed |= strip.effects.len() != before;
        }
        if removed {
            // Same reason a deleted track drops its lanes: the slot id outlives nothing, and a
            // curve addressed to a chain position that no longer exists is a curve that will
            // eventually be applied to whatever lands there.
            self.automation.remove_lanes_where(|target| {
                matches!(target, crate::param::ParamTarget::Effect { slot, .. } if slot == slot_id)
            });
        }
        removed
    }

    /// Removes a send from a track, returning `true` when it was there.
    ///
    /// Its automation goes with it, for the reason a deleted track's and a deleted effect slot's
    /// do: a lane addressed to a send that no longer exists would come back to life driving
    /// whichever send is created next.
    pub fn remove_send(&mut self, track: TrackId, send: SendId) -> bool {
        let Some(entry) = self.track_mut(track) else {
            return false;
        };
        let before = entry.sends.len();
        entry.sends.retain(|existing| existing.id != send);
        let removed = entry.sends.len() != before;
        if removed {
            self.automation.remove_lanes_where(|target| {
                matches!(target, crate::param::ParamTarget::Send { send: id, .. } if id == send)
            });
        }
        removed
    }

    /// `true` when any track is soloed, meaning non-soloed tracks must be silenced.
    pub fn has_solo(&self) -> bool {
        self.tracks.iter().any(|track| track.mixer.solo)
    }

    /// `true` when this track should be audible given the current mute/solo state.
    ///
    /// A convenience over [`Self::solo_resolution`] for a caller asking about one track; anything
    /// asking about all of them should call that instead and read the answers out of the vector,
    /// because a bus's answer depends on every track feeding it.
    pub fn track_is_audible(&self, track: &Track) -> bool {
        match self.track_index(track.id) {
            Some(index) => !track.mixer.mute && self.solo_resolution()[index],
            None => !track.mixer.mute && (!self.has_solo() || track.mixer.solo),
        }
    }

    /// Out-edges by track index: for each track, everything that cannot be mixed until it has.
    ///
    /// Two kinds of edge, and they are written down facing opposite ways. An [`Output`] and an
    /// [`AuxSend`] are recorded on the track the audio leaves, so they are read straight off. A
    /// sidechain is recorded on the slot that *listens*, because that is the end that wants one —
    /// so it is turned round here, and the track it names gains an edge to the track it keys.
    ///
    /// The master's own chain is left out on purpose: it runs after every track, so a sidechain
    /// keyed from anywhere is already waiting for it and there is nothing to order.
    fn routing_edges(&self) -> Vec<Vec<usize>> {
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); self.tracks.len()];
        for (index, track) in self.tracks.iter().enumerate() {
            for target in track.feeds() {
                if let Some(next) = self.track_index(target) {
                    edges[index].push(next);
                }
            }
            for source in track.mixer.sidechain_sources() {
                if let Some(from) = self.track_index(source)
                    && from != index
                {
                    edges[from].push(index);
                }
            }
        }
        edges
    }

    /// Track indices ordered so that everything feeding a bus comes before the bus.
    ///
    /// This is the order audio has to be mixed in: a bus cannot be put through its own strip until
    /// everything routed into it has arrived, and neither can a track whose compressor is keyed
    /// from another one. Every track appears exactly once even if the routing somehow holds a loop
    /// — see [`Self::repair_routing`] for why it should not — so a caller can walk this instead of
    /// the track list and know it has covered the project.
    pub fn routing_order(&self) -> Vec<usize> {
        // A depth-first post-order over the out-edges puts each node after everything it feeds;
        // reversing that puts it before them, which is the order wanted. A back edge is stepped
        // over rather than followed, so a loop costs an arbitrary starting point and not a hang.
        #[derive(Copy, Clone, PartialEq)]
        enum Mark {
            Unseen,
            Walking,
            Done,
        }
        let mut mark = vec![Mark::Unseen; self.tracks.len()];
        let mut order = Vec::with_capacity(self.tracks.len());
        let edges = self.routing_edges();
        // An explicit stack rather than recursion: the depth is the length of a routing chain,
        // which a document is free to make as long as it has tracks.
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for start in 0..self.tracks.len() {
            if mark[start] != Mark::Unseen {
                continue;
            }
            mark[start] = Mark::Walking;
            stack.push((start, 0));
            while let Some((node, edge)) = stack.pop() {
                match edges[node].get(edge).copied() {
                    Some(next) => {
                        stack.push((node, edge + 1));
                        if mark[next] == Mark::Unseen {
                            mark[next] = Mark::Walking;
                            stack.push((next, 0));
                        }
                    }
                    None => {
                        mark[node] = Mark::Done;
                        order.push(node);
                    }
                }
            }
        }
        order.reverse();
        order
    }

    /// `true` when routing `from` into `to` would make a signal loop back on itself.
    ///
    /// Asked before an output is changed or a send is added, because a loop is not a strange mix —
    /// it is a bus waiting for itself, and there is no order to render it in.
    ///
    /// Every edge already in the routing counts, sidechains included: a compressor keyed from a
    /// bus is the bus arriving early, and a track feeding that bus can no longer be rendered after
    /// it. Keying is asked about through [`Self::sidechain_would_cycle`], which is the same
    /// question with the ends the other way round.
    pub fn routing_would_cycle(&self, from: TrackId, to: TrackId) -> bool {
        if from == to {
            return true;
        }
        let (Some(from), Some(to)) = (self.track_index(from), self.track_index(to)) else {
            return false;
        };
        // The new edge closes a loop exactly when `from` is already downstream of `to`.
        let edges = self.routing_edges();
        let mut seen = vec![false; self.tracks.len()];
        seen[to] = true;
        let mut queue = vec![to];
        while let Some(node) = queue.pop() {
            for &next in &edges[node] {
                if next == from {
                    return true;
                }
                if !std::mem::replace(&mut seen[next], true) {
                    queue.push(next);
                }
            }
        }
        false
    }

    /// `true` when keying an effect on `owner` from `source` would make a signal loop back on
    /// itself.
    ///
    /// A sidechain is an edge from the source to the track it keys, so this is
    /// [`Self::routing_would_cycle`] with the ends swapped — and the swap is the whole reason it
    /// has a name of its own. Writing it out at each call site is writing down the one thing about
    /// sidechains that is easy to get backwards.
    pub fn sidechain_would_cycle(&self, owner: Option<TrackId>, source: TrackId) -> bool {
        match owner {
            // The master's chain runs after every track, so nothing it keys from can be waiting
            // for it.
            None => false,
            Some(owner) => self.routing_would_cycle(source, owner),
        }
    }

    /// Points every output and every send at a bus that exists, breaking any loop it finds.
    ///
    /// Called when a document is loaded, for the same reason [`Self::repair_id_counter`] is: the
    /// editing commands refuse to create either fault, so a file carrying one was either written
    /// by another tool or edited by hand, and the alternative to repairing it is a project that
    /// cannot be rendered at all. Returns `true` when anything had to be changed.
    pub fn repair_routing(&mut self) -> bool {
        let is_bus: Vec<(TrackId, bool)> = self
            .tracks
            .iter()
            .map(|track| (track.id, track.kind.is_bus()))
            .collect();
        let usable = |id: TrackId, owner: TrackId| {
            id != owner && is_bus.iter().any(|(bus, yes)| *bus == id && *yes)
        };

        let mut repaired = false;
        for index in 0..self.tracks.len() {
            let owner = self.tracks[index].id;
            if let Output::Bus(target) = self.tracks[index].output
                && !usable(target, owner)
            {
                self.tracks[index].output = Output::Master;
                repaired = true;
            }
            let before = self.tracks[index].sends.len();
            self.tracks[index]
                .sends
                .retain(|send| usable(send.target, owner));
            repaired |= self.tracks[index].sends.len() != before;
        }

        // Now every edge points at a real bus, so what is left is loops. Each edge is put back one
        // at a time and dropped if it closes one, which terminates and does not depend on where in
        // the document the loop happened to start.
        let edges: Vec<(usize, Output, Vec<AuxSend>)> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| (index, track.output, track.sends.clone()))
            .collect();
        for track in &mut self.tracks {
            track.output = Output::Master;
            track.sends.clear();
        }
        for (index, output, sends) in edges {
            let owner = self.tracks[index].id;
            if let Output::Bus(target) = output {
                if self.routing_would_cycle(owner, target) {
                    repaired = true;
                } else {
                    self.tracks[index].output = output;
                }
            }
            for send in sends {
                if self.routing_would_cycle(owner, send.target) {
                    repaired = true;
                } else {
                    self.tracks[index].sends.push(send);
                }
            }
        }

        // Sidechains last, and taken off before they are put back for the reason the edges above
        // are: a loop among them would otherwise be judged against itself. They are not held to
        // the bus rule — anything with a signal can key an effect — only to naming a track that
        // exists and not closing a loop.
        repaired |= self.repair_sidechains();
        repaired
    }

    /// Drops every sidechain that names a track which is not there, or which would leave a strip
    /// waiting for itself. Returns `true` when anything had to be changed.
    fn repair_sidechains(&mut self) -> bool {
        let mut repaired = false;
        let owners: Vec<Option<TrackId>> = std::iter::once(None)
            .chain(self.tracks.iter().map(|track| Some(track.id)))
            .collect();
        for owner in owners {
            let Some(strip) = self.strip(owner) else {
                continue;
            };
            let named: Vec<(usize, TrackId)> = strip
                .effects
                .iter()
                .enumerate()
                .filter_map(|(slot, effect)| effect.sidechain.map(|source| (slot, source)))
                .collect();
            for (slot, source) in named {
                let keep =
                    self.track(source).is_some() && !self.sidechain_would_cycle(owner, source);
                if keep {
                    continue;
                }
                if let Some(strip) = self.strip_mut(owner)
                    && let Some(effect) = strip.effects.get_mut(slot)
                {
                    effect.sidechain = None;
                    repaired = true;
                }
            }
        }
        repaired
    }

    /// The chain on a track, or the master's when `track` is `None`.
    fn strip(&self, track: Option<TrackId>) -> Option<&MixerStrip> {
        match track {
            Some(id) => self.track(id).map(|entry| &entry.mixer),
            None => Some(&self.master),
        }
    }

    /// The same, to be written to.
    fn strip_mut(&mut self, track: Option<TrackId>) -> Option<&mut MixerStrip> {
        match track {
            Some(id) => self.track_mut(id).map(|entry| &mut entry.mixer),
            None => Some(&mut self.master),
        }
    }

    /// Clears every sidechain keyed from `source`, wherever in the project it is.
    ///
    /// Called when a track is deleted. A slot left naming a track that is gone would key from
    /// silence, which is a compressor that has quietly stopped working — and would come back to
    /// life pointed at whichever track was created next, since ids are handed out in order.
    pub fn clear_sidechains_from(&mut self, source: TrackId) {
        for strip in self
            .tracks
            .iter_mut()
            .map(|track| &mut track.mixer)
            .chain(std::iter::once(&mut self.master))
        {
            for effect in &mut strip.effects {
                if effect.sidechain == Some(source) {
                    effect.sidechain = None;
                }
            }
        }
    }

    /// Which tracks the solo switches leave audible, in project order.
    ///
    /// This is the solo resolution alone; a track's own mute is separate, because the engine keeps
    /// the two apart so that toggling a mute is a command rather than a rebuild.
    ///
    /// A bus is what makes this more than one line. Solo has to travel **both ways along the
    /// routing**, or half of what a person means by it produces silence:
    ///
    /// * *Downstream*, because soloing a drum track routed through a drum bus must leave the bus
    ///   open — the track's audio has nowhere else to go.
    /// * *Upstream*, because soloing the drum bus must leave the drum tracks open — a bus has
    ///   nothing of its own to play, so a soloed bus with silenced feeders is silence.
    ///
    /// So a track is audible exactly when it lies on a path through something soloed. Two passes
    /// over [`Self::routing_order`] decide it: one forward for what a soloed track feeds, one
    /// backward for what feeds a soloed track. They stay separate on purpose — merging them would
    /// let a bus made audible from below drag in its *other* feeders, and soloing one drum track
    /// would quietly play the whole kit.
    pub fn solo_resolution(&self) -> Vec<bool> {
        if !self.has_solo() {
            return vec![true; self.tracks.len()];
        }
        self.audible_through(&|index| self.tracks[index].mixer.solo)
    }

    /// Which tracks are audible when `track` alone is soloed, whatever the solo switches say.
    ///
    /// A stem: one track and everywhere its audio has to travel to be heard. That is the same
    /// question [`Self::solo_resolution`] answers and is asked here of a solo nobody switched on,
    /// which is why the two share a walk — a stem worked out by a second, simpler rule would send
    /// a drum track routed through a drum bus to a silent file, and only on the projects that use
    /// buses.
    ///
    /// Every track when `track` is not in the project, for the reason a project with no solo is
    /// entirely audible: nothing was excluded.
    pub fn soloed_alone(&self, track: TrackId) -> Vec<bool> {
        match self.track_index(track) {
            Some(only) => self.audible_through(&|index| index == only),
            None => vec![true; self.tracks.len()],
        }
    }

    /// The solo walk, over whichever tracks `soloed` names.
    ///
    /// Two passes over [`Self::routing_order`], one each way, and they stay separate on purpose —
    /// merging them would let a bus made audible from below drag in its *other* feeders, and
    /// soloing one drum track would quietly play the whole kit.
    fn audible_through(&self, soloed: &dyn Fn(usize) -> bool) -> Vec<bool> {
        let order = self.routing_order();
        let index_of = |id: TrackId| self.track_index(id);

        // Backward: `reaches[t]` is true when something soloed sits downstream of `t`.
        let mut reaches = vec![false; self.tracks.len()];
        for &index in order.iter().rev() {
            reaches[index] = self.tracks[index].feeds().any(|target| {
                index_of(target).is_some_and(|target| soloed(target) || reaches[target])
            });
        }

        // Forward: `fed[t]` is true when something soloed sits upstream of `t`.
        let mut fed = vec![false; self.tracks.len()];
        for &index in &order {
            let id = self.tracks[index].id;
            fed[index] = self.tracks.iter().enumerate().any(|(feeder, other)| {
                (soloed(feeder) || fed[feeder]) && other.feeds().any(|target| target == id)
            });
        }

        (0..self.tracks.len())
            .map(|index| soloed(index) || reaches[index] || fed[index])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::fixtures::bussed_project;

    #[test]
    fn a_stem_carries_the_buses_its_track_is_routed_through() {
        // A drum track's stem is the drum track *and the drum bus*: the audio has nowhere else to
        // come out, and a stem that silenced the bus would be a silent file.
        let (project, kick, snare, bus) = bussed_project();
        let audible = project.soloed_alone(kick);
        let at = |id| project.track_index(id).expect("in the project");
        assert!(audible[at(kick)]);
        assert!(audible[at(bus)], "the bus the kick feeds was silenced");
        assert!(
            !audible[at(snare)],
            "the other track was in the kick's stem"
        );
    }

    #[test]
    fn a_stem_of_a_track_that_is_gone_leaves_everything_audible() {
        let (project, ..) = bussed_project();
        assert_eq!(
            project.soloed_alone(TrackId(9_999)),
            vec![true; project.tracks.len()]
        );
    }

    #[test]
    fn a_bus_is_ordered_after_everything_that_feeds_it() {
        // The order audio has to be mixed in: a bus cannot go through its own strip until every
        // track routed into it has arrived.
        let (mut project, kick, snare, bus) = bussed_project();
        let reverb = project.add_bus_track("Reverb");
        let send = project.next_send_id();
        project
            .track_mut(bus)
            .unwrap()
            .sends
            .push(AuxSend::new(send, reverb));

        let order = project.routing_order();
        assert_eq!(order.len(), project.tracks.len());
        let at = |id: TrackId| {
            let index = project.track_index(id).unwrap();
            order.iter().position(|slot| *slot == index).unwrap()
        };
        assert!(at(kick) < at(bus));
        assert!(at(snare) < at(bus));
        assert!(at(bus) < at(reverb), "a send is a routing edge too");
    }

    #[test]
    fn routing_refuses_to_close_a_loop() {
        let (mut project, kick, _, bus) = bussed_project();
        let reverb = project.add_bus_track("Reverb");
        project
            .track_mut(bus)
            .unwrap()
            .sends
            .push(AuxSend::new(SendId(500), reverb));

        // A bus into itself, and the two ways round the existing chain.
        assert!(project.routing_would_cycle(bus, bus));
        assert!(
            project.routing_would_cycle(reverb, bus),
            "reverb -> drums -> reverb"
        );
        assert!(
            project.routing_would_cycle(reverb, kick),
            "kick is upstream"
        );
        // And the edges that are simply new.
        assert!(!project.routing_would_cycle(kick, reverb));
    }

    #[test]
    fn a_document_carrying_a_loop_is_repaired_rather_than_refused() {
        // The editing commands cannot make one, so a file with a loop came from somewhere else.
        // The alternative to repairing it is a project that can never be rendered at all.
        let (mut project, _, _, bus) = bussed_project();
        let reverb = project.add_bus_track("Reverb");
        project.track_mut(bus).unwrap().output = Output::Bus(reverb);
        project.track_mut(reverb).unwrap().output = Output::Bus(bus);

        assert!(project.repair_routing());
        // One of the two edges survives — which one does not matter, only that no track can now
        // reach itself, which is what makes an order to render in exist.
        for track in &project.tracks {
            for target in track.feeds() {
                assert!(
                    !project.routing_would_cycle(track.id, target),
                    "{} still lies on a loop",
                    track.name
                );
            }
        }
        assert!(!project.repair_routing(), "the repair is idempotent");
        // And the bus is still reachable at all: repairing must not detach the whole chain.
        assert!(project.track(bus).unwrap().feeds().count() <= 1);
    }

    #[test]
    fn a_route_to_something_that_is_not_a_bus_is_dropped_on_load() {
        let (mut project, kick, snare, _) = bussed_project();
        // An instrument track is not a mixing point, and neither is a track that is not there.
        project.track_mut(kick).unwrap().output = Output::Bus(snare);
        project
            .track_mut(snare)
            .unwrap()
            .sends
            .push(AuxSend::new(SendId(9), TrackId(9_999)));

        assert!(project.repair_routing());
        assert_eq!(project.track(kick).unwrap().output, Output::Master);
        assert!(project.track(snare).unwrap().sends.is_empty());
    }

    #[test]
    fn solo_travels_both_ways_along_the_routing() {
        let (mut project, kick, snare, bus) = bussed_project();

        // Downstream: soloing a track has to leave the bus it feeds open, or its audio has
        // nowhere to go and the solo is silence.
        project.track_mut(kick).unwrap().mixer.solo = true;
        let audible = project.solo_resolution();
        let at = |id: TrackId| audible[project.track_index(id).unwrap()];
        assert!(at(kick));
        assert!(at(bus), "the soloed track's own bus was silenced");
        assert!(!at(snare));

        // Upstream: soloing the bus has to leave its feeders open, because a bus has nothing of
        // its own to play.
        project.track_mut(kick).unwrap().mixer.solo = false;
        project.track_mut(bus).unwrap().mixer.solo = true;
        let audible = project.solo_resolution();
        let at = |id: TrackId| audible[project.track_index(id).unwrap()];
        assert!(at(kick) && at(snare) && at(bus));
    }

    #[test]
    fn nothing_soloed_leaves_everything_audible() {
        let (project, _, _, _) = bussed_project();
        assert_eq!(project.solo_resolution(), vec![true; project.tracks.len()]);
    }

    #[test]
    fn deleting_a_bus_sends_what_fed_it_straight_to_the_master() {
        // The routing goes, not the tracks: what was going through the bus goes where it would
        // have gone had the bus never existed.
        let (mut project, kick, snare, bus) = bussed_project();
        project
            .track_mut(snare)
            .unwrap()
            .sends
            .push(AuxSend::new(SendId(77), bus));

        assert!(project.remove_track(bus));
        assert_eq!(project.track(kick).unwrap().output, Output::Master);
        assert!(project.track(snare).unwrap().sends.is_empty());
    }

    #[test]
    fn routing_survives_a_round_trip_through_json() {
        let (mut project, kick, _, bus) = bussed_project();
        project.track_mut(kick).unwrap().sends.push(AuxSend {
            id: SendId(42),
            target: bus,
            level_db: -7.5,
            pre_fader: true,
        });

        let json = serde_json::to_string(&project).expect("serialises");
        let back: Project = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, project, "{json}");
    }

    /// Keys the first effect on `owner` from `source`, and hands back the slot.
    fn key_from(project: &mut Project, owner: Option<TrackId>, source: TrackId) -> EffectSlotId {
        let slot = project.add_effect(owner, "auris.fx.compressor").unwrap();
        let strip = match owner {
            Some(id) => &mut project.track_mut(id).unwrap().mixer,
            None => &mut project.master,
        };
        strip.effects.last_mut().unwrap().sidechain = Some(source);
        slot
    }

    #[test]
    fn a_track_that_keys_another_is_ordered_before_it() {
        // The edge is written down on the bass and runs from the kick, which is the one thing
        // about a sidechain that is easy to get backwards.
        let (mut project, kick, _, _) = bussed_project();
        let bass = project.add_instrument_track("Bass", "x");
        key_from(&mut project, Some(bass), kick);

        let order = project.routing_order();
        assert_eq!(order.len(), project.tracks.len());
        let at = |id: TrackId| {
            let index = project.track_index(id).unwrap();
            order.iter().position(|slot| *slot == index).unwrap()
        };
        assert!(
            at(kick) < at(bass),
            "the key has to be there before it is read"
        );
    }

    #[test]
    fn keying_from_a_track_downstream_would_close_a_loop() {
        let (mut project, kick, _, bus) = bussed_project();
        // The kick already feeds the drum bus, so the bus cannot key an effect on the kick: it
        // would have to be mixed before the track it is waiting for.
        assert!(project.sidechain_would_cycle(Some(kick), bus));
        // The other way round is the ordinary case, and so is keying from a track that only
        // shares the master.
        assert!(!project.sidechain_would_cycle(Some(bus), kick));
        let bass = project.add_instrument_track("Bass", "x");
        assert!(!project.sidechain_would_cycle(Some(bass), kick));
        // An effect cannot key from the track it is sitting on.
        assert!(project.sidechain_would_cycle(Some(kick), kick));
        // The master runs last, so nothing it keys from can be waiting for it.
        assert!(!project.sidechain_would_cycle(None, bus));
    }

    #[test]
    fn an_existing_key_is_an_edge_the_next_route_has_to_clear() {
        // Keying the bass from the kick puts the kick upstream of the bass, and the bass's own
        // output carries that on to the bus it feeds. Keying the kick from *that* bus is then a
        // loop, though no output or send names both ends of it.
        let mut project = Project::new("Demo", 48_000.0);
        let kick = project.add_instrument_track("Kick", "x");
        let bass = project.add_instrument_track("Bass", "x");
        let bus = project.add_bus_track("Sub");
        project.track_mut(bass).unwrap().output = Output::Bus(bus);
        assert!(!project.sidechain_would_cycle(Some(kick), bus));

        key_from(&mut project, Some(bass), kick);
        assert!(
            project.sidechain_would_cycle(Some(kick), bus),
            "kick keys bass, bass feeds bus, so the bus cannot key the kick"
        );
    }

    #[test]
    fn a_key_that_cannot_be_rendered_is_dropped_on_load() {
        let (mut project, kick, _, bus) = bussed_project();
        // Three faults a document can carry that the editing commands cannot make: a key from a
        // track that is not there, a key from the track the effect sits on, and a key that would
        // leave a strip waiting for itself.
        let gone = key_from(&mut project, Some(kick), TrackId(9_999));
        let itself = key_from(&mut project, Some(kick), kick);
        let looped = key_from(&mut project, Some(kick), bus);
        let good = key_from(&mut project, Some(bus), kick);

        assert!(project.repair_routing());
        let keyed = |slot: EffectSlotId| {
            project
                .tracks
                .iter()
                .flat_map(|track| &track.mixer.effects)
                .find(|effect| effect.id == slot)
                .and_then(|effect| effect.sidechain)
        };
        assert_eq!(keyed(gone), None);
        assert_eq!(keyed(itself), None);
        assert_eq!(keyed(looped), None);
        assert_eq!(
            keyed(good),
            Some(kick),
            "the one that was fine is left alone"
        );
        assert!(!project.repair_routing(), "the repair is idempotent");
    }

    #[test]
    fn deleting_a_track_takes_the_keys_read_from_it_with_it() {
        // Ids are handed out in order, so a slot left holding a deleted track's id would come
        // back to life keyed from whichever track was created next.
        let (mut project, kick, snare, bus) = bussed_project();
        let from_kick = key_from(&mut project, Some(bus), kick);
        let on_master = key_from(&mut project, None, kick);
        let from_snare = key_from(&mut project, Some(bus), snare);

        assert!(project.remove_track(kick));
        let keyed = |slot: EffectSlotId| {
            project
                .tracks
                .iter()
                .map(|track| &track.mixer)
                .chain(std::iter::once(&project.master))
                .flat_map(|strip| &strip.effects)
                .find(|effect| effect.id == slot)
                .and_then(|effect| effect.sidechain)
        };
        assert_eq!(keyed(from_kick), None);
        assert_eq!(keyed(on_master), None, "the master's chain is cleared too");
        assert_eq!(keyed(from_snare), Some(snare));
    }

    #[test]
    fn solo_overrides_unsoloed_tracks() {
        let mut project = Project::new("Demo", 48_000.0);
        let a = project.add_instrument_track("A", "x");
        let b = project.add_instrument_track("B", "x");
        assert!(project.track_is_audible(project.track(a).unwrap()));

        project.track_mut(b).unwrap().mixer.solo = true;
        assert!(!project.track_is_audible(project.track(a).unwrap()));
        assert!(project.track_is_audible(project.track(b).unwrap()));
    }
}
