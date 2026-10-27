//! Preparation-only input for conventional native pipeline graphs.
//!
//! The compatibility snapshot stays private and cannot become a second document authority.
//! Its complete node address table travels with each graph, including when tracks are reordered.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use auris_core::pipeline::content::PluginParameter;
use auris_core::pipeline::{AutomationPolicy, NodeId};
use auris_core::project::{AudioSourceBank, EffectSlotId, Project, SendId, TrackId, TrackKind};
use auris_core::registry::PluginRegistry;
use auris_core::{Effect, Instrument, ParamDescriptor, ParamId, PluginKind};

use crate::{EngineError, OfflineOptions, OfflineRender, RenderGraph};

#[cfg(test)]
mod tests;

/// A persistent conventional object controlled by one native processor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeRenderTarget {
    /// Track strip, or the master strip when absent.
    Strip(Option<TrackId>),
    /// Software instrument, including a singer's guide instrument.
    Instrument(TrackId),
    /// Singer processor, distinct from its guide instrument.
    Singer(TrackId),
    /// Effect in a track or master chain.
    Effect {
        /// Owner track, or master.
        track: Option<TrackId>,
        /// Persistent effect slot.
        slot: EffectSlotId,
    },
    /// Auxiliary send owned by a track.
    Send {
        /// Owner track.
        track: TrackId,
        /// Persistent send.
        send: SendId,
    },
}

/// An address within exactly one prepared graph, for diagnostics rather than queued controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeNodeAddress {
    /// Track strip, or master.
    Strip {
        /// Track position, or master.
        track: Option<usize>,
    },
    /// Instrument position.
    Instrument {
        /// Track position.
        track: usize,
    },
    /// Singer position, which does not accept guide-instrument commands.
    Singer {
        /// Track position.
        track: usize,
    },
    /// Effect position.
    Effect {
        /// Track position, or master.
        track: Option<usize>,
        /// Position in the effect chain.
        slot: usize,
    },
    /// Send position.
    Send {
        /// Track position.
        track: usize,
        /// Position in the track's sends.
        send: usize,
    },
}

/// Immutable mapping owned by a prepared input or graph.
#[derive(Clone, Debug, Default)]
pub struct NativeRuntimeAddresses {
    by_node: BTreeMap<NodeId, RuntimeNodeAddress>,
    by_target: BTreeMap<NativeRenderTarget, NodeId>,
    continuous_parameters: BTreeSet<(NodeId, ParamId)>,
    pub(crate) generation: u64,
}

impl NativeRuntimeAddresses {
    /// Looks up a node without allocating or locking.
    pub fn address(&self, node: NodeId) -> Option<RuntimeNodeAddress> {
        self.by_node.get(&node).copied()
    }

    /// Finds the persistent node for a conventional command target.
    pub fn node_for_target(&self, target: NativeRenderTarget) -> Option<NodeId> {
        self.by_target.get(&target).copied()
    }

    /// Visits mutable nodes in stable identity order.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, RuntimeNodeAddress)> + '_ {
        self.by_node.iter().map(|(node, address)| (*node, *address))
    }

    pub(crate) fn accepts_parameter(&self, node: NodeId, param: ParamId) -> bool {
        self.continuous_parameters.contains(&(node, param))
    }
}

/// A stable-key lookup resolved for one graph generation, safe to send to audio.
///
/// Obtain this through [`RenderGraph::native_parameter`] on the preparation thread. A handle
/// from an earlier graph is rejected even when its node still exists in the replacement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeParameterHandle {
    pub(crate) node: NodeId,
    pub(crate) param: ParamId,
    pub(crate) generation: u64,
    pub(crate) min: f32,
    pub(crate) max: f32,
}

/// Identity of one successfully prepared native graph within this process.
///
/// Compare the candidate's [`RenderGraph::native_generation`] with the engine handle's adopted
/// generation before switching readers or controls. This is runtime evidence, not a saved
/// document revision; each rebuild receives a new identity even for the same document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NativeGraphGeneration(u64);

impl NativeGraphGeneration {
    pub(crate) fn from_raw(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }

    pub(crate) fn raw(self) -> u64 {
        self.0
    }
}

/// Caller-built effects keyed by native processor identity.
pub type NodePlacedEffects = BTreeMap<NodeId, Box<dyn Effect>>;
/// Caller-built instruments keyed by native processor identity.
pub type NodePlacedInstruments = BTreeMap<NodeId, Box<dyn Instrument>>;

/// Failure before a new native graph can be published.
#[derive(Debug, thiserror::Error)]
pub enum NativeRenderError {
    /// Snapshot identities or routes are inconsistent.
    #[error("invalid conventional render snapshot: {0}")]
    Snapshot(&'static str),
    /// Mutable targets are not covered by a one-to-one node mapping.
    #[error("invalid native runtime bindings: {0}")]
    Bindings(&'static str),
    /// A supplied plugin does not belong to this node or match its contract.
    #[error("native plugin {node:?}: {reason}")]
    Plugin {
        /// Processor that failed validation.
        node: NodeId,
        /// Specific placement or metadata failure.
        reason: String,
    },
    /// The requested preparation block size exceeds the bounded renderer budget.
    #[error("invalid native render block size: {0}")]
    BlockSize(usize),
    /// Graph generations have exhausted their non-reusable identity space.
    #[error("native render generation space exhausted")]
    GenerationExhausted,
    /// Scheduling, rate or offline geometry failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

/// A private conventional snapshot and its validated native identities.
///
/// This value has no document accessor or serialization path. Rebuild it from the native
/// document after an edit; only the prepared graph crosses into the audio thread.
pub struct NativeRenderInput {
    project: Project,
    addresses: NativeRuntimeAddresses,
    parameter_contracts: BTreeMap<NodeId, Vec<PluginParameter>>,
}

impl NativeRenderInput {
    /// Consumes an ephemeral snapshot and requires exactly one node per mutable target.
    ///
    /// `contracts` must cover every instrument and effect with the descriptors and control
    /// policies used during lowering. Only continuous parameters receive realtime handles.
    /// The live objects must match the descriptors before the graph is built.
    pub fn new(
        project: Project,
        bindings: Vec<(NodeId, NativeRenderTarget)>,
        contracts: BTreeMap<NodeId, Vec<PluginParameter>>,
    ) -> Result<Self, NativeRenderError> {
        if project.conflicting_object_id().is_some() {
            return Err(NativeRenderError::Snapshot("duplicate object identity"));
        }
        if !project.routing_is_valid() {
            return Err(NativeRenderError::Snapshot("invalid or cyclic routing"));
        }
        let expected = expected_addresses(&project);
        let mut addresses = NativeRuntimeAddresses::default();
        for (node, target) in bindings {
            let address = expected
                .get(&target)
                .copied()
                .ok_or(NativeRenderError::Bindings(
                    "unknown target or incorrect owner",
                ))?;
            if addresses.by_node.insert(node, address).is_some()
                || addresses.by_target.insert(target, node).is_some()
            {
                return Err(NativeRenderError::Bindings("duplicate node or target"));
            }
        }
        if addresses.by_target.len() != expected.len() {
            return Err(NativeRenderError::Bindings("missing mutable target"));
        }
        let mut input = Self {
            project,
            addresses,
            parameter_contracts: contracts,
        };
        let expected: BTreeSet<_> = input
            .addresses
            .iter()
            .filter_map(|(node, address)| {
                matches!(
                    address,
                    RuntimeNodeAddress::Instrument { .. } | RuntimeNodeAddress::Effect { .. }
                )
                .then_some(node)
            })
            .collect();
        if input
            .parameter_contracts
            .keys()
            .copied()
            .collect::<BTreeSet<_>>()
            != expected
        {
            return Err(NativeRenderError::Bindings(
                "incomplete plugin parameter contracts",
            ));
        }
        input.addresses.continuous_parameters = input
            .parameter_contracts
            .iter()
            .flat_map(|(node, parameters)| {
                parameters.iter().filter_map(move |parameter| {
                    (parameter.policy == AutomationPolicy::Continuous
                        && !parameter.descriptor.read_only
                        && parameter.descriptor.min < parameter.descriptor.max)
                        .then_some((*node, parameter.descriptor.id))
                })
            })
            .collect();
        input.validate_automation()?;
        Ok(input)
    }

    /// Reads the identities resolved for this input, without exposing its snapshot.
    pub fn addresses(&self) -> &NativeRuntimeAddresses {
        &self.addresses
    }

    /// Builds a complete graph on the preparation thread.
    ///
    /// Initial placements, identities and metadata are checked before any supplied object is
    /// taken. Scheduling failures or descriptor changes during state loading/preparation may
    /// consume render halves; they never publish a partial graph. The caller retains the
    /// currently playing graph until this succeeds.
    pub fn build(
        &self,
        bank: &AudioSourceBank,
        registry: &PluginRegistry,
        effects: &mut NodePlacedEffects,
        instruments: &mut NodePlacedInstruments,
        max_block: usize,
        sample_rate: f64,
    ) -> Result<RenderGraph, NativeRenderError> {
        if !(1..=crate::offline::MAX_BLOCK_FRAMES).contains(&max_block) {
            return Err(NativeRenderError::BlockSize(max_block));
        }
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(EngineError::InvalidSampleRate(sample_rate).into());
        }
        for node in effects.keys() {
            if !matches!(
                self.addresses.address(*node),
                Some(RuntimeNodeAddress::Effect { .. })
            ) {
                return Err(plugin_error(
                    *node,
                    "effect placement has no matching effect node",
                ));
            }
        }
        for node in instruments.keys() {
            if !matches!(
                self.addresses.address(*node),
                Some(RuntimeNodeAddress::Instrument { .. })
            ) {
                return Err(plugin_error(
                    *node,
                    "instrument placement has no matching instrument node",
                ));
            }
        }
        let mut prepared_effects = crate::PlacedEffects::new();
        let mut prepared_instruments = crate::PlacedInstruments::new();
        for (target, node) in &self.addresses.by_target {
            match *target {
                NativeRenderTarget::Effect { track, slot } => {
                    let strip = track.map_or(&self.project.master, |id| {
                        &self.project.track(id).expect("validated target").mixer
                    });
                    let slot = strip
                        .effects
                        .iter()
                        .find(|entry| entry.id == slot)
                        .expect("validated target");
                    if let Some(effect) = effects.get(node) {
                        self.check_effect(*node, slot, effect.as_ref())?;
                    } else {
                        if slot.file.is_some() {
                            return Err(plugin_error(
                                *node,
                                "hosted effect render half is missing",
                            ));
                        }
                        let effect = registry
                            .create_effect(&slot.effect_id)
                            .map_err(|error| plugin_error(*node, error.to_string()))?;
                        self.check_effect(*node, slot, effect.as_ref())?;
                        prepared_effects.insert(slot.id, effect);
                    }
                }
                NativeRenderTarget::Instrument(track) => {
                    let kind = &self.project.track(track).expect("validated target").kind;
                    let (id, hosted) = match kind {
                        TrackKind::Instrument(inner) | TrackKind::Drum(inner) => {
                            (&inner.instrument_id, inner.file.is_some())
                        }
                        TrackKind::Singer(inner) => (&inner.instrument_id, false),
                        _ => unreachable!("validated instrument target"),
                    };
                    if let Some(instrument) = instruments.get(node) {
                        self.check_plugin(
                            *node,
                            id,
                            PluginKind::Instrument,
                            instrument.descriptor(),
                            instrument.parameters(),
                        )?;
                    } else {
                        if hosted {
                            return Err(plugin_error(
                                *node,
                                "hosted instrument render half is missing",
                            ));
                        }
                        let instrument = registry
                            .create_instrument(id)
                            .map_err(|error| plugin_error(*node, error.to_string()))?;
                        self.check_plugin(
                            *node,
                            id,
                            PluginKind::Instrument,
                            instrument.descriptor(),
                            instrument.parameters(),
                        )?;
                        prepared_instruments.insert(track, instrument);
                    }
                }
                _ => {}
            }
        }
        let generation = next_generation()?;
        for (target, node) in &self.addresses.by_target {
            match *target {
                NativeRenderTarget::Effect { slot, .. } => {
                    if let Some(effect) = effects.remove(node) {
                        prepared_effects.insert(slot, effect);
                    }
                }
                NativeRenderTarget::Instrument(track) => {
                    if let Some(instrument) = instruments.remove(node) {
                        prepared_instruments.insert(track, instrument);
                    }
                }
                _ => {}
            }
        }
        let mut graph = RenderGraph::build_with(
            &self.project,
            bank,
            registry,
            &mut prepared_effects,
            &mut prepared_instruments,
            max_block,
            sample_rate,
        );
        if let Some(error) = graph.take_resource_error() {
            return Err(error.into());
        }
        // Loading state and preparing a hosted object can refresh its descriptors. Recheck the
        // exact objects that will render, before publishing automation resolved against them.
        for (node, expected) in &self.parameter_contracts {
            let parameters = match self.addresses.address(*node).expect("validated contract") {
                RuntimeNodeAddress::Instrument { track } => match &graph.tracks[track].source {
                    crate::RenderSource::Instrument { instrument, .. }
                    | crate::RenderSource::Sung { instrument, .. } => instrument.parameters(),
                    _ => return Err(plugin_error(*node, "prepared instrument is unavailable")),
                },
                RuntimeNodeAddress::Effect { track, slot } => {
                    let strip = track.map_or(&graph.master, |index| &graph.tracks[index].strip);
                    strip.effects[slot].parameters()
                }
                _ => unreachable!("validated parameter contract"),
            };
            if !descriptors_match(expected, parameters) {
                return Err(plugin_error(
                    *node,
                    "parameters changed while preparing the plugin",
                ));
            }
        }
        graph.native_addresses = self.addresses.clone();
        graph.native_addresses.generation = generation;
        Ok(graph)
    }

    /// Prepares an offline pass through the same node-validated graph as realtime playback.
    pub fn offline(
        &self,
        bank: &AudioSourceBank,
        registry: &PluginRegistry,
        effects: &mut NodePlacedEffects,
        instruments: &mut NodePlacedInstruments,
        options: &OfflineOptions,
    ) -> Result<OfflineRender, NativeRenderError> {
        let graph = self.build(
            bank,
            registry,
            effects,
            instruments,
            options
                .block_frames
                .clamp(1, crate::offline::MAX_BLOCK_FRAMES),
            options.sample_rate.unwrap_or(self.project.sample_rate),
        )?;
        Ok(OfflineRender::from_graph(&self.project, graph, options)?)
    }

    fn check_effect(
        &self,
        node: NodeId,
        slot: &auris_core::project::EffectSlot,
        effect: &dyn Effect,
    ) -> Result<(), NativeRenderError> {
        self.check_plugin(
            node,
            &slot.effect_id,
            PluginKind::Effect,
            effect.descriptor(),
            effect.parameters(),
        )?;
        if slot.sidechain.is_some() && !effect.wants_sidechain() {
            return Err(plugin_error(
                node,
                "effect does not accept its connected sidechain",
            ));
        }
        Ok(())
    }

    fn validate_automation(&self) -> Result<(), NativeRenderError> {
        use auris_core::ParamTarget;
        for lane in self.project.automation.lanes() {
            let (target, parameter) = match lane.target {
                ParamTarget::TrackGain(track) | ParamTarget::TrackPan(track) => {
                    (NativeRenderTarget::Strip(Some(track)), None)
                }
                ParamTarget::MasterGain | ParamTarget::MasterPan => {
                    (NativeRenderTarget::Strip(None), None)
                }
                ParamTarget::Send { track, send } => {
                    (NativeRenderTarget::Send { track, send }, None)
                }
                ParamTarget::Instrument { track, param } => {
                    (NativeRenderTarget::Instrument(track), Some(param))
                }
                ParamTarget::Effect { track, slot, param } => {
                    (NativeRenderTarget::Effect { track, slot }, Some(param))
                }
            };
            let node =
                self.addresses
                    .node_for_target(target)
                    .ok_or(NativeRenderError::Snapshot(
                        "automation targets an absent object",
                    ))?;
            let builtin = auris_core::param::mixer_descriptor(lane.target);
            let descriptor = match parameter {
                Some(param) => self.parameter_contracts[&node]
                    .get(param.index())
                    .filter(|parameter| parameter.policy == AutomationPolicy::Continuous)
                    .map(|parameter| &parameter.descriptor)
                    .filter(|descriptor| {
                        descriptor.id == param && Some(descriptor.key.as_ref()) == lane.key()
                    }),
                None => builtin.as_ref(),
            }
            .ok_or_else(|| plugin_error(node, "automation key and runtime parameter disagree"))?;
            if descriptor.read_only
                || lane.points().iter().any(|point| {
                    !point.value.is_finite()
                        || !(descriptor.min..=descriptor.max).contains(&point.value)
                })
            {
                return Err(plugin_error(
                    node,
                    "automation exceeds the writable parameter domain",
                ));
            }
        }
        Ok(())
    }

    fn check_plugin(
        &self,
        node: NodeId,
        id: &str,
        kind: PluginKind,
        descriptor: auris_core::PluginDescriptor,
        parameters: &[ParamDescriptor],
    ) -> Result<(), NativeRenderError> {
        if descriptor.id != id || descriptor.kind != kind {
            return Err(plugin_error(
                node,
                "plugin identity or kind differs from the document",
            ));
        }
        let mut keys = BTreeSet::new();
        if parameters.iter().enumerate().any(|(index, parameter)| {
            parameter.id.index() != index
                || !keys.insert(&parameter.key)
                || !parameter.min.is_finite()
                || !parameter.max.is_finite()
                || parameter.min > parameter.max
        }) {
            return Err(plugin_error(node, "invalid runtime parameter descriptors"));
        }
        if self
            .parameter_contracts
            .get(&node)
            .is_some_and(|expected| !descriptors_match(expected, parameters))
        {
            return Err(plugin_error(
                node,
                "runtime parameters differ from lowering capabilities",
            ));
        }
        Ok(())
    }
}

fn plugin_error(node: NodeId, reason: impl Into<String>) -> NativeRenderError {
    NativeRenderError::Plugin {
        node,
        reason: reason.into(),
    }
}

fn descriptors_match(expected: &[PluginParameter], actual: &[ParamDescriptor]) -> bool {
    expected.len() == actual.len()
        && expected
            .iter()
            .zip(actual)
            .all(|(expected, actual)| expected.descriptor == *actual)
}

fn next_generation() -> Result<u64, NativeRenderError> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut current = NEXT.load(Ordering::Relaxed);
    loop {
        let next = current
            .checked_add(1)
            .ok_or(NativeRenderError::GenerationExhausted)?;
        match NEXT.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(current),
            Err(observed) => current = observed,
        }
    }
}

fn expected_addresses(project: &Project) -> BTreeMap<NativeRenderTarget, RuntimeNodeAddress> {
    let mut addresses = BTreeMap::new();
    addresses.insert(
        NativeRenderTarget::Strip(None),
        RuntimeNodeAddress::Strip { track: None },
    );
    for (index, track) in project.tracks.iter().enumerate() {
        addresses.insert(
            NativeRenderTarget::Strip(Some(track.id)),
            RuntimeNodeAddress::Strip { track: Some(index) },
        );
        if track.kind.holds_notes() {
            addresses.insert(
                NativeRenderTarget::Instrument(track.id),
                RuntimeNodeAddress::Instrument { track: index },
            );
        }
        if matches!(track.kind, TrackKind::Singer(_)) {
            addresses.insert(
                NativeRenderTarget::Singer(track.id),
                RuntimeNodeAddress::Singer { track: index },
            );
        }
        for (send_index, send) in track.sends.iter().enumerate() {
            addresses.insert(
                NativeRenderTarget::Send {
                    track: track.id,
                    send: send.id,
                },
                RuntimeNodeAddress::Send {
                    track: index,
                    send: send_index,
                },
            );
        }
    }
    for (track, strip) in project
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| (Some((index, track.id)), &track.mixer))
        .chain(std::iter::once((None, &project.master)))
    {
        for (index, slot) in strip.effects.iter().enumerate() {
            addresses.insert(
                NativeRenderTarget::Effect {
                    track: track.map(|(_, id)| id),
                    slot: slot.id,
                },
                RuntimeNodeAddress::Effect {
                    track: track.map(|(index, _)| index),
                    slot: index,
                },
            );
        }
    }
    addresses
}
