//! Model-facing graph operations shared by saved-file and live-document transports.

use serde_json::{Value, json};

use super::{MixerEdgeKind, MixerNode, destination};
use crate::{Edit, ParamTarget, Session, prelude::*};

#[cfg(test)]
mod tests;

/// A connection's role; every role participates in DAG validation.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    /// A copy of the track's signal into a bus or the master, at a level and tap of its own.
    Output,
    /// Feed an effect's detector input.
    Sidechain,
    /// Notes from a score, or a performance, into an instrument or a performance.
    Notes,
}

/// One graph operation, using stable node IDs returned by mixer_graph read.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphAction {
    /// Replace all outputs atomically with unity post-fader outputs to these targets. Empty targets explicitly disconnects the source; levels and pre-fader outputs are not kept.
    SetOutputs {
        /// Stable source track node.
        source: String,
        /// Stable bus track nodes or track:master, at most 64.
        #[schemars(length(max = 64))]
        targets: Vec<String>,
    },
    /// Add one output with its level and tap in one Undo step, keeping every existing output. A track holds at most one output per target and tap; refused if this one exists (use set_output).
    AddOutput {
        /// Stable source track node.
        source: String,
        /// Stable destination bus node or track:master.
        target: String,
        /// Initial level, -60..0 dB.
        #[schemars(range(min = -60, max = 0))]
        gain_db: f32,
        /// True taps after inserts before the fader; false (the usual) after the fader.
        pre_fader: bool,
    },
    /// Replace or clear an effect's sidechain atomically.
    SetSidechain {
        /// Stable effect node.
        node: String,
        /// Stable source track node; null clears the sidechain.
        source: Option<String>,
    },
    /// Set fader/pan/mute/solo on a stable track node. Supply at least one control; omitted controls stay unchanged. track:master accepts gain_db and pan. Static values may be overridden by automation. The edit is one undo step.
    SetStrip {
        /// Exact track node ID, including track:master.
        node: String,
        /// Static fader value, -60..12 decibels.
        #[schemars(range(min = -60, max = 12))]
        gain_db: Option<f32>,
        /// Stereo pan, -1..1.
        #[schemars(range(min = -1, max = 1))]
        pan: Option<f32>,
        /// Mute this track; omitted preserves it. Not available on master.
        mute: Option<bool>,
        /// Solo this track additively; omitted preserves it. Not available on master.
        solo: Option<bool>,
    },
    /// Read the mixer DAG: stable node IDs, outgoing edges, controls and available effects. Nodes are paged in processing order; follow next_offset. Parallel processing branches through buses. Internal chain edges are changed by effect ordering, not connect/disconnect.
    Read {
        /// Zero-based first node, default 0.
        #[serde(default)]
        offset: usize,
        /// Node page size, default 32, clamped to 1..64.
        limit: Option<usize>,
    },
    /// Connect two nodes from mixer_graph read. source must be a track output; output targets a bus track node or master and adds a unity post-fader output; sidechain targets an effect; notes runs from a score:N node to an instrument:M node, adding that score to what the instrument plays (its own, or a melodic or singer score onto a melodic instrument). All cycles are refused, including mixed output/sidechain paths. Duplicate connections are unchanged. Changes are undoable.
    Connect {
        /// Exact track node ID or its output_node from mixer_graph read.
        source: String,
        /// Exact track or effect node ID from mixer_graph read.
        target: String,
        /// output, sidechain or notes; must match the target port.
        kind: ConnectionKind,
    },
    /// Disconnect an existing graph edge. Only the named output, sidechain or notes edge is removed; an empty output list remains disconnected, and a layer whose only notes path is cut keeps what it played as clips of its own. Internal chain edges cannot be disconnected. Supply pre_fader when a pre- and a post-fader output share the same source/target. A stale or mismatched edge is refused.
    Disconnect {
        /// Exact source track node ID, including for a pre-fader output.
        source: String,
        /// Exact destination track or effect node ID.
        target: String,
        /// Role of the edge to remove.
        kind: ConnectionKind,
        /// Which output to remove by its tap; only accepted for kind output.
        pre_fader: Option<bool>,
    },
    /// Add an effect before a strip's fader. input is the strip's track node ID; effect is an exact available_effects ID. Returns the new effect node ID. Use a bus branch for parallel effects.
    AddEffect {
        /// Input node ID, including track:master for the master strip.
        input: String,
        /// Exact available_effects ID returned by mixer_graph read.
        effect: String,
    },
    /// Remove an effect node and its automation. Other effect node IDs stay valid.
    RemoveEffect {
        /// Exact effect node ID.
        node: String,
    },
    /// Move an effect within its strip's serial chain. Node IDs remain stable after ordering changes.
    MoveEffect {
        /// Exact effect node ID.
        node: String,
        /// New 1-based chain position.
        #[schemars(range(min = 1))]
        position: usize,
    },
    /// Enable or bypass an effect node without removing its connections or state.
    EnableEffect {
        /// Exact effect node ID.
        node: String,
        /// Whether the effect processes audio.
        enabled: bool,
    },
    /// Set an existing output's static level and/or move its tap. The output is named by source, target and pre_fader. At least one of gain_db and move_pre_fader is required. Automation can override the static level; inspect the returned state.
    SetOutput {
        /// Stable source track node.
        source: String,
        /// Stable destination bus node or track:master.
        target: String,
        /// Which output: true for the one taken before the fader. Default false.
        #[serde(default)]
        pre_fader: bool,
        /// Static level in decibels, -60..0.
        #[schemars(range(min = -60, max = 0))]
        gain_db: Option<f32>,
        /// Move the output: true takes it after inserts but before the fader, false after it.
        move_pre_fader: Option<bool>,
    },
    /// Read an effect node's parameter keys, value_domain, units, ranges, steps, read_only flags, static values and automation flags. VST3 values are normalized 0..1, not display units such as Hz/dB. Follow next_offset for more parameters. Choice labels are previews of at most 16.
    Parameters {
        /// Exact effect node ID.
        node: String,
        /// Zero-based first parameter, default 0.
        #[serde(default)]
        offset: usize,
        /// Page size, default 32, clamped to 1..64.
        limit: Option<usize>,
    },
    /// Set one static parameter on an effect node using a key and value_domain from mixer_graph parameters (VST3: normalized 0..1). Refuses read_only controls, non-finite or out-of-range values and invalid steps. Automation, if present, still overrides the static value.
    SetParameter {
        /// Exact effect node ID.
        node: String,
        /// Exact parameter key from mixer_graph parameters.
        param: String,
        /// Value in the parameter's native units.
        value: f32,
    },
}

impl GraphAction {
    /// Operation name within the single mixer_graph tool.
    pub fn operation_name(&self) -> &'static str {
        match self {
            Self::SetOutputs { .. } => "set_outputs",
            Self::AddOutput { .. } => "add_output",
            Self::SetSidechain { .. } => "set_sidechain",
            Self::Read { .. } => "read",
            Self::Connect { .. } => "connect",
            Self::Disconnect { .. } => "disconnect",
            Self::AddEffect { .. } => "add_effect",
            Self::RemoveEffect { .. } => "remove_effect",
            Self::MoveEffect { .. } => "move_effect",
            Self::EnableEffect { .. } => "enable_effect",
            Self::SetOutput { .. } => "set_output",
            Self::Parameters { .. } => "parameters",
            Self::SetParameter { .. } => "set_parameter",
            Self::SetStrip { .. } => "set_strip",
        }
    }

    /// Whether the operation preserves the document.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Read { .. } | Self::Parameters { .. })
    }

    /// Whether the ordinary live-agent policy asks before removing content or connections.
    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self,
            Self::Disconnect { .. }
                | Self::RemoveEffect { .. }
                | Self::SetOutputs { .. }
                | Self::SetSidechain { .. }
        )
    }
}

impl MixerNode {
    /// Stable model-facing identity, independent of names and effect positions.
    pub fn key(self) -> String {
        match self {
            Self::Strip(owner) => format!(
                "track:{}",
                owner.map_or_else(|| "master".into(), |id| id.0.to_string())
            ),

            Self::Effect(_, slot) => format!("effect:{}", slot.0),
            Self::Performance(id) => format!("performance:{}", id.0),
            Self::Score(track) => format!("score:{}", track.0),
            Self::Instrument(track) => format!("instrument:{}", track.0),
        }
    }
}

impl Session {
    pub(crate) fn graph_node(&self, key: &str) -> Result<MixerNode, String> {
        self.mixer_graph()
            .nodes
            .into_iter()
            .find(|node| node.key() == key)
            .ok_or_else(|| format!("Unknown mixer node '{key}'; mixer_graph read again"))
    }

    fn graph_effect(&self, key: &str) -> Result<(Option<TrackId>, EffectSlotId), String> {
        match self.graph_node(key)? {
            MixerNode::Effect(owner, slot) => Ok((owner, slot)),
            _ => Err("Expected an effect node ID from mixer_graph read".into()),
        }
    }

    fn graph_chain(&self, owner: Option<TrackId>) -> &[EffectSlot] {
        owner
            .and_then(|id| self.project().track(id))
            .map_or(&self.project().master, |track| &track.mixer)
            .effects
            .as_slice()
    }

    fn graph_parameter_targets(
        &mut self,
        node: &str,
    ) -> Result<Vec<(ParamTarget, ParamDescriptor)>, String> {
        let (owner, slot) = self.graph_effect(node)?;
        Ok(self
            .effect_descriptors(slot)
            .iter()
            .map(|descriptor| {
                (
                    ParamTarget::Effect {
                        track: owner,
                        slot,
                        param: descriptor.id,
                    },
                    descriptor.clone(),
                )
            })
            .collect())
    }

    /// Read or edit the mixer through stable graph identities. Validates before recording history.
    /// The caller owns persistence; live edits remain unsaved and saved-file tools checkpoint once.
    pub fn apply_mixer_graph(&mut self, operation: &GraphAction) -> Result<Value, String> {
        let error = |error: crate::SessionError| error.to_string();
        let mut changed = false;
        let mut result = Value::Null;
        match operation {
            GraphAction::SetOutputs { source, targets } => {
                if targets.len() > 64 {
                    return Err("At most 64 output destinations".into());
                }
                let MixerNode::Strip(Some(track)) = self.graph_node(source)? else {
                    return Err("Expected source track node".into());
                };
                let outputs = targets
                    .iter()
                    .map(|target| {
                        let (_, destination) =
                            self.graph_connection(source, target, ConnectionKind::Output)?;
                        match destination {
                            MixerNode::Strip(owner) => Ok(Output::new(super::destination(owner))),
                            _ => Err("Expected bus or master track node".to_string()),
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let revision = self.revision();
                self.set_track_outputs(track, &outputs).map_err(error)?;
                changed = self.revision() != revision;
            }
            GraphAction::AddOutput {
                source,
                target,
                gain_db,
                pre_fader,
            } => {
                if !(-60.0..=0.0).contains(gain_db) {
                    return Err("gain_db must be finite and within -60..0".into());
                }
                let (track, to) = self.graph_output_target(source, target)?;
                let tap = tap_of(*pre_fader);
                let mut outputs = self.project().outputs(track);
                if outputs.iter().any(|output| output.key() == (to, tap)) {
                    return Err(
                        "This output already exists; use set_output to change its level".into(),
                    );
                }
                outputs.push(Output {
                    to,
                    gain_db: *gain_db,
                    tap,
                });
                self.set_track_outputs(track, &outputs).map_err(error)?;
                changed = true;
                result = json!({"source":source,"target":target,"gain_db":gain_db,"pre_fader":pre_fader});
            }
            GraphAction::SetSidechain { node, source } => {
                let (owner, slot) = self.graph_effect(node)?;
                let source = source
                    .as_ref()
                    .map(|source| {
                        self.graph_connection(source, node, ConnectionKind::Sidechain)
                            .map(|(track, _)| track)
                    })
                    .transpose()?;
                changed = self.effect_sidechain(owner, slot) != source;
                if changed {
                    self.set_effect_sidechain(owner, slot, source)
                        .map_err(error)?;
                }
            }
            GraphAction::SetStrip {
                node,
                gain_db,
                pan,
                mute,
                solo,
            } => {
                let MixerNode::Strip(owner) = self.graph_node(node)? else {
                    return Err("Expected a track node ID".into());
                };
                if gain_db.is_none() && pan.is_none() && mute.is_none() && solo.is_none() {
                    return Err("Supply at least one track control".into());
                }
                if gain_db.is_some_and(|gain| !(-60.0..=12.0).contains(&gain))
                    || pan.is_some_and(|pan| !(-1.0..=1.0).contains(&pan))
                {
                    return Err(
                        "gain_db must be finite and within -60..12; pan within -1..1".into(),
                    );
                }
                if owner.is_none() && (mute.is_some() || solo.is_some()) {
                    return Err("Master accepts gain_db and pan; mute/solo require a track".into());
                }
                let strip = owner
                    .and_then(|id| self.project().track(id))
                    .map_or(&self.project().master, |track| &track.mixer);
                changed = gain_db.is_some_and(|value| value != strip.gain_db)
                    || pan.is_some_and(|value| value != strip.pan)
                    || mute.is_some_and(|value| value != strip.mute)
                    || solo.is_some_and(|value| value != strip.solo);
                if changed {
                    self.begin_transaction(Edit::ExternalChanges);
                    if let Some(gain) = gain_db {
                        self.set_param(
                            owner.map_or(ParamTarget::MasterGain, ParamTarget::TrackGain),
                            *gain,
                        );
                    }
                    if let Some(pan) = pan {
                        self.set_param(
                            owner.map_or(ParamTarget::MasterPan, ParamTarget::TrackPan),
                            *pan,
                        );
                    }
                    let update = if let Some(track) = owner {
                        mute.map_or(Ok(()), |value| self.set_track_mute(track, value))
                            .and_then(|()| {
                                solo.map_or(Ok(()), |value| self.set_track_solo(track, value))
                            })
                    } else {
                        Ok(())
                    };
                    self.end_transaction();
                    update.map_err(error)?;
                }
                let strip = owner
                    .and_then(|id| self.project().track(id))
                    .map_or(&self.project().master, |track| &track.mixer);
                result = json!({"node":node,"gain_db":strip.gain_db,"pan":strip.pan,"mute":strip.mute,"solo":strip.solo});
            }
            GraphAction::Read { offset, limit } => return self.mixer_graph_report(*offset, *limit),
            GraphAction::Parameters {
                node,
                offset,
                limit,
            } => {
                let parameters = self.graph_parameter_targets(node)?;
                if *offset > parameters.len() {
                    return Err("offset exceeds parameter count".into());
                }
                let page: Vec<_> = parameters
                    .iter()
                    .skip(*offset)
                    .take(limit.unwrap_or(32).clamp(1, 64))
                    .map(|(target, descriptor)| self.agent_parameter_report(*target, descriptor))
                    .collect();
                let next = offset + page.len();
                return Ok(
                    json!({"node":node,"parameters":page,"total":parameters.len(),"next_offset":(next < parameters.len()).then_some(next)}),
                );
            }
            GraphAction::Connect {
                source,
                target,
                kind: ConnectionKind::Notes,
            } => {
                let (MixerNode::Score(from), MixerNode::Instrument(to)) =
                    (self.graph_node(source)?, self.graph_node(target)?)
                else {
                    return Err(
                        "A notes connection runs from a score node to an instrument node".into(),
                    );
                };
                if !self.project().routing.has_notes_edge(
                    auris_core::NotesNode::Score(from),
                    auris_core::NotesNode::Instrument(to),
                ) {
                    if !self.can_connect_score(from, to) {
                        return Err("Connection refused: that instrument may not play this score (a drum score plays only its own kit, a layer's score is empty, and only a melodic instrument plays another track's)".into());
                    }
                    changed = self.connect_score(from, to).map_err(error)?;
                }
            }
            GraphAction::Disconnect {
                source,
                target,
                kind: ConnectionKind::Notes,
                pre_fader,
            } => {
                if pre_fader.is_some() {
                    return Err("pre_fader is only accepted for an output edge".into());
                }
                let (Some(from), Some(to)) = (
                    self.graph_node(source)?.notes(),
                    self.graph_node(target)?.notes(),
                ) else {
                    return Err("A notes edge joins score, performance and instrument nodes".into());
                };
                changed = self.disconnect_notes(from, to).map_err(error)?;
                if !changed {
                    return Err("This notes edge no longer exists; mixer_graph read again".into());
                }
            }
            GraphAction::Connect {
                source,
                target,
                kind,
            } => {
                let (from, to) = self.graph_connection(source, target, *kind)?;
                let exists = match kind {
                    ConnectionKind::Output => {
                        let key = match to {
                            MixerNode::Strip(owner) => (destination(owner), Tap::PostFader),
                            _ => (Destination::Master, Tap::PostFader),
                        };
                        self.project()
                            .routing
                            .outputs(from)
                            .any(|output| output.key() == key)
                    }
                    ConnectionKind::Sidechain => match to {
                        MixerNode::Effect(owner, slot) => {
                            self.effect_sidechain(owner, slot) == Some(from)
                        }
                        _ => false,
                    },
                    ConnectionKind::Notes => unreachable!("handled above"),
                };
                if !exists {
                    if !self.can_connect_mixer(from, to) {
                        return Err("Connection refused: incompatible port or a cycle through outputs or sidechains".into());
                    }
                    changed = self.connect_mixer(from, to).map_err(error)?;
                }
            }
            GraphAction::Disconnect {
                source,
                target,
                kind,
                pre_fader,
            } => {
                let (from, to) = self.graph_connection(source, target, *kind)?;
                if *kind != ConnectionKind::Output && pre_fader.is_some() {
                    return Err("pre_fader is only accepted for an output edge".into());
                }
                match kind {
                    ConnectionKind::Output => {
                        let to = match to {
                            MixerNode::Strip(owner) => destination(owner),
                            _ => return Err("Expected an input".into()),
                        };
                        let matching: Vec<Tap> = self
                            .project()
                            .routing
                            .outputs(from)
                            .filter(|output| {
                                output.to == to
                                    && pre_fader.is_none_or(|pre| output.tap == tap_of(pre))
                            })
                            .map(|output| output.tap)
                            .collect();
                        match matching.as_slice() {
                            [] => {
                                return Err(
                                    "This output edge no longer exists; mixer_graph read again"
                                        .into(),
                                );
                            }
                            [tap] => {
                                self.remove_track_output(from, to, *tap).map_err(error)?;
                                changed = true;
                            }
                            _ => {
                                return Err("A pre- and a post-fader output share this source and target; supply pre_fader".into());
                            }
                        }
                    }
                    ConnectionKind::Sidechain => {
                        let (owner, slot) = self.graph_effect(target)?;
                        if self.effect_sidechain(owner, slot) != Some(from) {
                            return Err(
                                "This sidechain edge no longer exists; mixer_graph read again"
                                    .into(),
                            );
                        }
                        self.set_effect_sidechain(owner, slot, None)
                            .map_err(error)?;
                        changed = true;
                    }
                    ConnectionKind::Notes => unreachable!("handled above"),
                }
            }
            GraphAction::AddEffect { input, effect } => {
                let MixerNode::Strip(owner) = self.graph_node(input)? else {
                    return Err("input must be a track node ID".into());
                };
                let slot = self.add_effect(owner, effect).map_err(error)?;
                result = json!({"node":MixerNode::Effect(owner, slot).key(),"slot_id":slot.0});
                changed = true;
            }
            GraphAction::RemoveEffect { node } => {
                let (_, slot) = self.graph_effect(node)?;
                self.remove_effect(slot);
                changed = true;
                result = json!({"removed_node":node});
            }
            GraphAction::MoveEffect { node, position } => {
                let (owner, slot) = self.graph_effect(node)?;
                let chain = self.graph_chain(owner);
                if !(1..=chain.len()).contains(position) {
                    return Err("position must be inside the effect chain (1-based)".into());
                }
                let index = chain
                    .iter()
                    .position(|effect| effect.id == slot)
                    .ok_or("Unknown effect node")?;
                let delta = *position as isize - 1 - index as isize;
                changed = delta != 0;
                if changed {
                    self.move_effect(owner, slot, delta);
                }
                result = json!({"node":node,"position":position});
            }
            GraphAction::EnableEffect { node, enabled } => {
                let (owner, slot) = self.graph_effect(node)?;
                changed = self.effect_enabled(owner, slot) != Some(*enabled);
                if changed {
                    self.set_effect_enabled(owner, slot, *enabled);
                }
                result = json!({"node":node,"enabled":enabled});
            }
            GraphAction::SetOutput {
                source,
                target,
                pre_fader,
                gain_db,
                move_pre_fader,
            } => {
                if gain_db.is_none() && move_pre_fader.is_none() {
                    return Err("Supply gain_db and/or move_pre_fader".into());
                }
                if gain_db.is_some_and(|gain| !(-60.0..=0.0).contains(&gain)) {
                    return Err("gain_db must be finite and within -60..0".into());
                }
                let (track, to) = self.graph_output_target(source, target)?;
                let tap = tap_of(*pre_fader);
                let output = self
                    .project()
                    .routing
                    .outputs(track)
                    .find(|output| output.key() == (to, tap))
                    .copied()
                    .ok_or("This output does not exist; mixer_graph read again")?;
                let moved = move_pre_fader.map_or(tap, tap_of);
                changed = gain_db.is_some_and(|value| value != output.gain_db) || moved != tap;
                if changed {
                    self.begin_transaction(Edit::ExternalChanges);
                    let applied = gain_db
                        .map_or(Ok(()), |gain| self.set_output_gain(track, to, tap, gain))
                        .and_then(|()| self.set_output_tap(track, to, tap, moved));
                    // Closed either way: an open transaction disables Undo and autosave.
                    match applied {
                        Ok(()) => {
                            self.end_transaction();
                        }
                        Err(failure) => {
                            self.revert_transaction();
                            return Err(error(failure));
                        }
                    }
                }
                result = json!({"source":source,"target":target,"gain_db":gain_db.unwrap_or(output.gain_db),"pre_fader":moved == Tap::PreFader,"automated":self.is_automated(ParamTarget::Output {track, to, tap: moved})});
            }
            GraphAction::SetParameter { node, param, value } => {
                let parameters = self.graph_parameter_targets(node)?;
                let (target, descriptor) = parameters
                    .iter()
                    .find(|(_, descriptor)| descriptor.key.as_ref() == param)
                    .ok_or("Unknown parameter key; use mixer_graph parameters")?;
                if descriptor.read_only {
                    return Err("parameter is read-only".into());
                }
                if !value.is_finite() || !(descriptor.min..=descriptor.max).contains(value) {
                    return Err(
                        "value must be finite and within the parameter's native range".into(),
                    );
                }
                if (descriptor.clamp(*value) - *value).abs() > 1e-5 {
                    return Err(
                        "value must match a native discrete choice or parameter step".into(),
                    );
                }
                changed = self.param_value(*target, descriptor) != *value;
                if changed {
                    self.set_param(*target, *value);
                }
                result = json!({"node":node,"param":param,"value":self.param_value(*target, descriptor),"automated":self.is_automated(*target)});
            }
        }
        Ok(json!({"changed":changed,"result":result,"graph":self.mixer_graph_report(0, None)?}))
    }

    fn graph_connection(
        &self,
        source: &str,
        target: &str,
        kind: ConnectionKind,
    ) -> Result<(TrackId, MixerNode), String> {
        let from = match self.graph_node(source)? {
            MixerNode::Strip(Some(from)) => from,
            node @ MixerNode::Effect(Some(from), _)
                if self.mixer_graph_output(Some(from)) == node =>
            {
                from
            }
            _ => {
                return Err(
                    "source must be a track or its terminal effect node; the master is a sink"
                        .into(),
                );
            }
        };
        let to = self.graph_node(target)?;
        match (kind, to) {
            (ConnectionKind::Output, MixerNode::Strip(owner)) => {
                if owner.is_some_and(|id| {
                    self.project()
                        .track(id)
                        .is_none_or(|track| !track.kind.is_bus())
                }) {
                    return Err("Only bus and master inputs accept routing connections".into());
                }
            }
            (ConnectionKind::Sidechain, MixerNode::Effect(_, _)) => {}
            _ => return Err("Connection kind does not match the target port".into()),
        }
        Ok((from, to))
    }

    /// The source track and destination of an output named by graph node IDs.
    fn graph_output_target(
        &self,
        source: &str,
        target: &str,
    ) -> Result<(TrackId, Destination), String> {
        match self.graph_connection(source, target, ConnectionKind::Output)? {
            (track, MixerNode::Strip(owner)) => Ok((track, destination(owner))),
            _ => Err("Expected bus or master track node".into()),
        }
    }

    /// A bounded JSON graph page, in processing order, with each page node's outgoing edges.
    pub fn mixer_graph_report(
        &mut self,
        offset: usize,
        limit: Option<usize>,
    ) -> Result<Value, String> {
        let graph = self.mixer_graph();
        let owners = self
            .project()
            .routing_order()
            .into_iter()
            .map(|index| Some(self.project().tracks[index].id))
            .chain(std::iter::once(None));
        // Strips and their inserts in processing order, then the performances, which belong to no
        // strip and are resolved before playback.
        let ordered: Vec<_> = owners
            .flat_map(|owner| {
                graph
                    .nodes
                    .iter()
                    .filter(move |node| node.owner() == Some(owner))
                    .copied()
            })
            .chain(
                graph
                    .nodes
                    .iter()
                    .filter(|node| node.owner().is_none())
                    .copied(),
            )
            .collect();
        if offset > ordered.len() {
            return Err("offset exceeds mixer node count".into());
        }
        let page: Vec<_> = ordered
            .iter()
            .copied()
            .skip(offset)
            .take(limit.unwrap_or(32).clamp(1, 64))
            .collect();
        let mut nodes = Vec::new();
        for node in &page {
            if let MixerNode::Performance(id) = node {
                let performance = self
                    .project()
                    .routing
                    .performance(*id)
                    .ok_or("Unknown performance")?;
                nodes.push(json!({"id":node.key(),"kind":"performance","name":performance.name,"transforms":performance.transforms}));
                continue;
            }
            let owner = node.owner().flatten();
            let track = owner.and_then(|id| self.project().track(id));
            let strip = track.map_or(&self.project().master, |track| &track.mixer);
            let mut row = json!({"id":node.key(),"track_id":owner.map(|id| id.0),"name":track.map_or("master",|track| track.name.as_str())});
            match node {
                MixerNode::Strip(_) => {
                    row["kind"] = "track".into();
                    row["accepts_routing"] =
                        (owner.is_none() || track.is_some_and(|track| track.kind.is_bus())).into();
                    row["controls"] = json!({"gain_db":strip.gain_db,"pan":strip.pan,"mute":strip.mute,"solo":strip.solo});
                    row["gain_automated"] = self
                        .is_automated(owner.map_or(ParamTarget::MasterGain, ParamTarget::TrackGain))
                        .into();
                    row["pan_automated"] = self
                        .is_automated(owner.map_or(ParamTarget::MasterPan, ParamTarget::TrackPan))
                        .into();
                    row["output_node"] = self.mixer_graph_output(owner).key().into();
                }
                MixerNode::Effect(_, slot) => {
                    let index = strip
                        .effects
                        .iter()
                        .position(|effect| effect.id == *slot)
                        .ok_or("Unknown effect")?;
                    row["kind"] = "effect".into();
                    row["effect_id"] = strip.effects[index].effect_id.clone().into();
                    row["slot_id"] = slot.0.into();
                    row["position"] = (index + 1).into();
                    row["enabled"] = strip.effects[index].enabled.into();
                    row["accepts_sidechain"] = self.effect_wants_sidechain(owner, *slot).into();
                }
                MixerNode::Score(_) => {
                    row["kind"] = "score".into();
                    row["clips"] = track
                        .and_then(|track| track.kind.note_clips())
                        .map_or(0, Vec::len)
                        .into();
                }
                MixerNode::Instrument(_) => {
                    row["kind"] = "instrument".into();
                    row["instrument_id"] = track
                        .and_then(|track| match &track.kind {
                            auris_core::TrackKind::Singer(inner) => Some(&inner.instrument_id),
                            kind => kind.as_instrument().map(|inner| &inner.instrument_id),
                        })
                        .cloned()
                        .into();
                    row["layer"] = owner
                        .is_some_and(|id| self.project().routing.is_layer(id))
                        .into();
                }
                MixerNode::Performance(_) => unreachable!("reported above"),
            }
            // People's notes about this node; edit_comments with mixer_node changes them.
            let comments = self.comments(crate::session::CommentHost::Mixer(*node));
            if !comments.is_empty() {
                row["comments"] = comments
                    .iter()
                    .map(|comment| json!({"comment":comment.id.0,"text":comment.text}))
                    .collect::<Vec<_>>()
                    .into();
            }
            nodes.push(row);
        }
        let edges: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| page.contains(&edge.source))
            .map(|edge| {
                // What the edge carries, so a reader can tell notes from audio without knowing
                // which kinds carry which.
                let signal = match edge.kind {
                    MixerEdgeKind::Notes => "notes",
                    _ => "audio",
                };
                let mut row =
                    json!({"source":edge.source.key(),"target":edge.target.key(),"signal":signal});
                match edge.kind {
                    MixerEdgeKind::Chain => row["kind"] = "chain".into(),
                    MixerEdgeKind::Sidechain => row["kind"] = "sidechain".into(),
                    MixerEdgeKind::Notes => row["kind"] = "notes".into(),
                    MixerEdgeKind::Output {
                        track,
                        gain_db,
                        tap,
                    } => {
                        row["kind"] = "output".into();
                        row["gain_db"] = gain_db.into();
                        row["pre_fader"] = (tap == Tap::PreFader).into();
                        row["control_source"] = MixerNode::Strip(Some(track)).key().into();
                        let to = match edge.target {
                            MixerNode::Strip(owner) => destination(owner),
                            _ => Destination::Master,
                        };
                        row["automated"] = self
                            .is_automated(ParamTarget::Output { track, to, tap })
                            .into();
                    }
                }
                row
            })
            .collect();
        let effects: Vec<_> = self
            .registry()
            .effects()
            .map(|effect| json!({"id":effect.id,"name":effect.name}))
            .collect();
        let next = offset + page.len();
        Ok(
            json!({"acyclic":true,"order":"processing","nodes":nodes,"edges":edges,"total_nodes":ordered.len(),"next_offset":(next < ordered.len()).then_some(next),"available_effects":effects}),
        )
    }
}

/// The tap a model-facing `pre_fader` flag names.
fn tap_of(pre_fader: bool) -> Tap {
    if pre_fader {
        Tap::PreFader
    } else {
        Tap::PostFader
    }
}
