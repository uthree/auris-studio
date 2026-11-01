//! Model-facing graph operations shared by saved-file and live-document transports.

use serde_json::{Value, json};

use super::{MixerEdgeKind, MixerNode};
use crate::{Edit, ParamTarget, Session, prelude::*};

#[cfg(test)]
mod tests;

/// A connection's role; every role participates in DAG validation.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    /// Add a unity post-fader output without replacing existing destinations.
    Output,
    /// Add an independent post-fader copy feeding a bus.
    Send,
    /// Feed an effect's detector input.
    Sidechain,
}

/// One graph operation, using stable node IDs returned by mixer_graph read.
#[derive(Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphAction {
    /// Replace all output destinations atomically. Empty targets explicitly disconnects the source.
    SetOutputs {
        /// Stable source track node.
        source: String,
        /// Stable bus track nodes or track:master, at most 64.
        #[schemars(length(max = 64))]
        targets: Vec<String>,
    },
    /// Create a send with its level and tap in one Undo step. Returns the stable send ID.
    AddSend {
        /// Stable source track node.
        source: String,
        /// Stable destination bus node.
        target: String,
        /// Initial level, -60..0 dB.
        #[schemars(range(min = -60, max = 0))]
        level_db: f32,
        /// True taps after inserts before the fader.
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
    /// Connect two nodes from mixer_graph read. source must be a track output; output/send target a bus track node (output also accepts master), sidechain targets an effect. All cycles are refused, including mixed output/send/sidechain paths. Duplicate connections are unchanged. Changes are undoable.
    Connect {
        /// Exact track node ID or its output_node from mixer_graph read.
        source: String,
        /// Exact track or effect node ID from mixer_graph read.
        target: String,
        /// output, send, or sidechain; must match the target port.
        kind: ConnectionKind,
    },
    /// Disconnect an existing graph edge. Only the named output, send or sidechain is removed; an empty output list remains disconnected. Internal chain edges cannot be disconnected. Supply send_id when several sends share the same source/target. A stale or mismatched edge is refused.
    Disconnect {
        /// Exact source track node ID, including for a pre-fader send.
        source: String,
        /// Exact destination track or effect node ID.
        target: String,
        /// Role of the edge to remove.
        kind: ConnectionKind,
        /// Existing send ID; only accepted for kind send.
        send_id: Option<u64>,
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
    /// Set an existing send's static level and/or pre-fader tap. At least one value is required. Automation can override the static level; inspect the returned state.
    SetSend {
        /// Stable send ID from a graph edge.
        send_id: u64,
        /// Static send level in decibels, -60..0.
        #[schemars(range(min = -60, max = 0))]
        level_db: Option<f32>,
        /// True taps after inserts but before the fader; false taps after the fader.
        pre_fader: Option<bool>,
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
            Self::AddSend { .. } => "add_send",
            Self::SetSidechain { .. } => "set_sidechain",
            Self::Read { .. } => "read",
            Self::Connect { .. } => "connect",
            Self::Disconnect { .. } => "disconnect",
            Self::AddEffect { .. } => "add_effect",
            Self::RemoveEffect { .. } => "remove_effect",
            Self::MoveEffect { .. } => "move_effect",
            Self::EnableEffect { .. } => "enable_effect",
            Self::SetSend { .. } => "set_send",
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
        }
    }
}

impl Session {
    fn graph_node(&self, key: &str) -> Result<MixerNode, String> {
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
                            MixerNode::Strip(owner) => {
                                Ok(owner.map_or(Output::Master, Output::Bus))
                            }
                            _ => Err("Expected bus or master track node".to_string()),
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let revision = self.revision();
                self.set_track_outputs(track, &outputs).map_err(error)?;
                changed = self.revision() != revision;
            }
            GraphAction::AddSend {
                source,
                target,
                level_db,
                pre_fader,
            } => {
                if !(-60.0..=0.0).contains(level_db) {
                    return Err("level_db must be finite and within -60..0".into());
                }
                let (track, destination) =
                    self.graph_connection(source, target, ConnectionKind::Send)?;
                let MixerNode::Strip(Some(bus)) = destination else {
                    return Err("Expected destination bus node".into());
                };
                self.begin_transaction(Edit::ExternalChanges);
                let applied = self.add_send(track, bus).and_then(|send| {
                    self.set_send_level(track, send, *level_db)?;
                    self.set_send_pre_fader(track, send, *pre_fader)?;
                    Ok(send)
                });
                let send = match applied {
                    Ok(send) => {
                        self.end_transaction();
                        send
                    }
                    Err(failure) => {
                        self.revert_transaction();
                        return Err(error(failure));
                    }
                };
                changed = true;
                result = json!({"send_id":send.0,"level_db":level_db,"pre_fader":pre_fader});
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
                kind,
            } => {
                let (from, to) = self.graph_connection(source, target, *kind)?;
                let exists = match kind {
                    ConnectionKind::Output => self.project().track(from).is_some_and(|track| {
                        track.outputs.contains(&match to {
                            MixerNode::Strip(owner) => owner.map_or(Output::Master, Output::Bus),
                            _ => Output::Master,
                        })
                    }),
                    ConnectionKind::Send => self.project().track(from).is_some_and(|track| {
                        track
                            .sends
                            .iter()
                            .any(|send| MixerNode::Strip(Some(send.target)) == to)
                    }),
                    ConnectionKind::Sidechain => match to {
                        MixerNode::Effect(owner, slot) => {
                            self.effect_sidechain(owner, slot) == Some(from)
                        }
                        _ => false,
                    },
                };
                if !exists {
                    if !self.can_connect_mixer(from, to, *kind == ConnectionKind::Send) {
                        return Err("Connection refused: incompatible port or a cycle through outputs, sends or sidechains".into());
                    }
                    changed = self
                        .connect_mixer(from, to, *kind == ConnectionKind::Send)
                        .map_err(error)?;
                }
            }
            GraphAction::Disconnect {
                source,
                target,
                kind,
                send_id,
            } => {
                let (from, to) = self.graph_connection(source, target, *kind)?;
                if *kind != ConnectionKind::Send && send_id.is_some() {
                    return Err("send_id is only accepted for a send edge".into());
                }
                match kind {
                    ConnectionKind::Output => {
                        let target = match to {
                            MixerNode::Strip(owner) => owner,
                            _ => return Err("Expected an input".into()),
                        };
                        if self.project().track(from).is_none_or(|track| {
                            !track
                                .outputs
                                .contains(&target.map_or(Output::Master, Output::Bus))
                        }) {
                            return Err(
                                "This output edge no longer exists; mixer_graph read again".into(),
                            );
                        }
                        changed = true;
                        self.remove_track_output(from, target.map_or(Output::Master, Output::Bus))
                            .map_err(error)?;
                    }
                    ConnectionKind::Send => {
                        let sends: Vec<_> = self
                            .project()
                            .track(from)
                            .into_iter()
                            .flat_map(|track| &track.sends)
                            .filter(|send| {
                                MixerNode::Strip(Some(send.target)) == to
                                    && send_id.is_none_or(|id| send.id.0 == id)
                            })
                            .map(|send| send.id)
                            .collect();
                        if sends.len() != 1 {
                            return Err("Expected exactly one matching send edge; use send_id from mixer_graph read".into());
                        }
                        self.remove_send(from, sends[0]).map_err(error)?;
                        changed = true;
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
            GraphAction::SetSend {
                send_id,
                level_db,
                pre_fader,
            } => {
                if level_db.is_none() && pre_fader.is_none() {
                    return Err("Supply level_db and/or pre_fader".into());
                }
                if level_db.is_some_and(|level| !(-60.0..=0.0).contains(&level)) {
                    return Err("level_db must be finite and within -60..0".into());
                }
                let (owner, send) = self
                    .project()
                    .tracks
                    .iter()
                    .find_map(|track| {
                        track
                            .sends
                            .iter()
                            .find(|send| send.id.0 == *send_id)
                            .map(|send| (track.id, send.clone()))
                    })
                    .ok_or("Unknown send_id; mixer_graph read again")?;
                changed = level_db.is_some_and(|value| value != send.level_db)
                    || pre_fader.is_some_and(|value| value != send.pre_fader);
                if changed {
                    self.begin_transaction(Edit::ExternalChanges);
                    let applied = level_db
                        .map_or(Ok(()), |level| self.set_send_level(owner, send.id, level))
                        .and_then(|()| {
                            pre_fader
                                .map_or(Ok(()), |pre| self.set_send_pre_fader(owner, send.id, pre))
                        });
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
                result = json!({"send_id":send_id,"level_db":level_db.unwrap_or(send.level_db),"pre_fader":pre_fader.unwrap_or(send.pre_fader),"automated":self.is_automated(ParamTarget::Send {track:owner,send:send.id})});
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
            (ConnectionKind::Output, MixerNode::Strip(owner))
            | (ConnectionKind::Send, MixerNode::Strip(owner)) => {
                if owner.is_none() && kind == ConnectionKind::Send {
                    return Err("A send must feed a bus track node".into());
                }
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
        let ordered: Vec<_> = owners
            .flat_map(|owner| {
                graph
                    .nodes
                    .iter()
                    .filter(move |node| match node {
                        MixerNode::Strip(id) | MixerNode::Effect(id, _) => *id == owner,
                    })
                    .copied()
            })
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
            let owner = match node {
                MixerNode::Strip(id) | MixerNode::Effect(id, _) => *id,
            };
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
            }
            nodes.push(row);
        }
        let edges: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| page.contains(&edge.source))
            .map(|edge| {
                let mut row = json!({"source":edge.source.key(),"target":edge.target.key()});
                match edge.kind {
                    MixerEdgeKind::Chain => row["kind"] = "chain".into(),
                    MixerEdgeKind::Output => row["kind"] = "output".into(),
                    MixerEdgeKind::Sidechain => row["kind"] = "sidechain".into(),
                    MixerEdgeKind::Notes => row["kind"] = "notes".into(),
                    MixerEdgeKind::Send {
                        track,
                        send,
                        level_db,
                        pre_fader,
                    } => {
                        row["kind"] = "send".into();
                        row["send_id"] = send.0.into();
                        row["level_db"] = level_db.into();
                        row["pre_fader"] = pre_fader.into();
                        row["control_source"] = MixerNode::Strip(Some(track)).key().into();
                        row["automated"] =
                            self.is_automated(ParamTarget::Send { track, send }).into();
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
