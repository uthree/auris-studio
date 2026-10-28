//! Native graph preparation with isolated host instances and adoption-bound editor ownership.
//!
//! This component owns its engine; a conventional [`crate::Session`] is not changed by using it.
//! All methods run on the main/control thread. The caller owns the native document and its
//! revision. Preparing, queueing and callback adoption are separate operations. Returned host
//! changes and snapshots must be reconciled with that document explicitly.

mod clap;
mod vst3;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use auris_clap::RawWindowHandle;
use auris_core::pipeline::content::{CapabilityProvider, ContentLimits, ProcessorKind};
use auris_core::pipeline::document::NativeDocument;
use auris_core::pipeline::{NodeId, PipelineLimits};
use auris_core::project::AudioSourceBank;
use auris_core::registry::PluginRegistry;
use auris_core::{Effect, Instrument, PluginState, PrepareContext};
use auris_engine::native::{NativeGraphGeneration, NodePlacedEffects, NodePlacedInstruments};
use auris_engine::{
    AudioDevice, AudioSettings, EngineCommand, EngineError, EngineHandle, RenderGraph,
};

use crate::PluginWindow;
use crate::native_conventional::{NativePreparationError, prepare_native_conventional};

/// A native host operation failed without publishing a partial host set.
#[derive(Debug, thiserror::Error)]
pub enum NativeHostError {
    /// Native topology or capability validation failed.
    #[error(transparent)]
    Preparation(#[from] NativePreparationError),
    /// Render preparation failed.
    #[error(transparent)]
    Render(#[from] auris_engine::native::NativeRenderError),
    /// The audio device or command queue rejected an operation.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// A CLAP instance rejected an operation.
    #[error(transparent)]
    Clap(#[from] auris_clap::ClapError),
    /// A VST3 instance rejected an operation.
    #[error(transparent)]
    Vst3(#[from] auris_vst3::Vst3Error),
    /// A node's plugin identity or role cannot be hosted.
    #[error("{0}")]
    InvalidPlugin(String),
    /// A plugin path was unresolved or outside configured plugin locations.
    #[error("plugin file for node {0:?} is unavailable or untrusted")]
    UntrustedPlugin(NodeId),
    /// An operation failed on a specific persistent processor.
    #[error("native host {node:?} during {operation}: {source}")]
    Node {
        /// Persistent processor identity.
        node: NodeId,
        /// Host operation that failed.
        operation: &'static str,
        /// Underlying host failure.
        source: Box<NativeHostError>,
    },
    /// One candidate is already waiting for callback adoption.
    #[error("a native candidate is waiting for callback adoption")]
    Pending,
    /// No fully prepared candidate is available to enqueue.
    #[error("no native candidate is prepared")]
    NotPrepared,
    /// The caller's document changed after candidate preparation.
    #[error("native candidate revision {prepared} is stale; current revision is {current}")]
    StaleRevision {
        /// Revision captured during preparation.
        prepared: u64,
        /// Revision the caller currently owns.
        current: u64,
    },
    /// A window or snapshot request did not name a published hosted node.
    #[error("native host {0:?} is not published")]
    NotPublished(NodeId),
}

impl NativeHostError {
    fn at(self, node: NodeId, operation: &'static str) -> Self {
        Self::Node {
            node,
            operation,
            source: Box::new(self),
        }
    }
}

/// Borrowed resources for preparing one immutable native document revision.
pub struct NativeHostResources<'a> {
    /// Already loaded audio assets for the snapshot.
    pub bank: &'a AudioSourceBank,
    /// Built-in instrument/effect factories.
    pub registry: &'a PluginRegistry,
    /// Project folder used to resolve stored hosted plugin paths.
    pub project_folder: Option<&'a Path>,
    /// Ownership validation budget.
    pub content_limits: ContentLimits,
    /// Topology and capability validation budget.
    pub graph_limits: PipelineLimits,
}

/// Actual host state with the document revision and graph generation it belongs to.
#[derive(Debug)]
pub struct NativeHostSnapshot {
    /// Persistent processor whose actual state was captured.
    pub node: NodeId,
    /// Native document revision supplied during preparation.
    pub revision: u64,
    /// Adopted graph owning this host instance.
    pub generation: NativeGraphGeneration,
    /// Opaque state and current readable parameter values.
    pub state: PluginState,
}

/// Control-thread observations from a native playback poll.
#[derive(Default, Debug)]
pub struct NativeHostUpdate {
    /// A candidate was promoted after exact callback adoption.
    pub adopted: Option<NativeGraphGeneration>,
    /// Published nodes reporting editor/source changes, scoped to their graph generation.
    pub changed: Vec<(NativeGraphGeneration, NodeId)>,
    /// Host, editor, or device failures, never silently treated as successful publication.
    pub errors: Vec<NativeHostError>,
}

enum RenderHalf {
    Effect(Box<dyn Effect>),
    Instrument(Box<dyn Instrument>),
}

enum Host {
    Clap(clap::ClapHost),
    Vst3(vst3::Vst3Host),
}

impl Host {
    fn service(&mut self) -> bool {
        match self {
            Self::Clap(host) => host.service(),
            Self::Vst3(host) => host.service(),
        }
    }

    fn release(&mut self) -> bool {
        match self {
            Self::Clap(host) => host.release(),
            Self::Vst3(host) => host.release(),
        }
    }

    fn window_is_open(&self) -> bool {
        match self {
            Self::Clap(host) => host.window_is_open(),
            Self::Vst3(host) => host.window_is_open(),
        }
    }

    fn has_window(&mut self) -> bool {
        match self {
            Self::Clap(host) => host.has_window(),
            Self::Vst3(host) => host.has_window(),
        }
    }

    fn set_window_open(
        &mut self,
        open: bool,
        parent: Option<RawWindowHandle>,
    ) -> Result<(), NativeHostError> {
        match self {
            Self::Clap(host) => host.set_window_open(open, parent),
            Self::Vst3(host) => host.set_window_open(open, parent),
        }
    }

    fn snapshot(&mut self) -> Result<PluginState, NativeHostError> {
        match self {
            Self::Clap(host) => host.snapshot(),
            Self::Vst3(host) => host.snapshot(),
        }
    }
}

struct HostGeneration {
    generation: NativeGraphGeneration,
    revision: u64,
    hosts: BTreeMap<NodeId, Host>,
    windows: BTreeMap<PluginWindow, NodeId>,
}

// Declaration order is part of ownership: audio halves must drop before main-thread instances.
struct Prepared {
    graph: RenderGraph,
    owners: HostGeneration,
}

#[derive(Default)]
struct Placement {
    effects: NodePlacedEffects,
    instruments: NodePlacedInstruments,
    hosts: BTreeMap<NodeId, Host>,
}

/// Native playback engine and main-thread hosted instances.
///
/// Only one queued candidate is permitted. Further preparation waits for adoption or device
/// failure, bounding pending host ownership. Fresh candidate instances intentionally trade load
/// cost for isolation; the conventional session's live/spare maps are never reused here.
/// Device, queued/retired graphs, then host owners are dropped in that order.
pub struct NativeHostedPlayback {
    device: AudioDevice,
    engine: EngineHandle,
    prepared: Option<Prepared>,
    pending: Option<HostGeneration>,
    published: Option<HostGeneration>,
    retiring: Vec<Host>,
    trust: crate::session::plugin_trust::PluginTrust,
    parent: Option<RawWindowHandle>,
    open_windows: BTreeSet<NodeId>,
    output_was_running: bool,
}

impl NativeHostedPlayback {
    /// Opens a real output device, returning the hardware error instead of a silent fallback.
    /// Additional trusted plugin paths have the same folder/file semantics as Session settings.
    pub fn start(
        settings: &AudioSettings,
        plugin_paths: &[PathBuf],
    ) -> Result<Self, NativeHostError> {
        let (device, engine) = auris_engine::try_start_audio(settings)?;
        Ok(Self::from_engine(device, engine, plugin_paths))
    }

    /// Creates a control-thread preparation environment without opening hardware.
    /// Queueing is rejected because a silent engine cannot acknowledge callback adoption.
    pub fn silent(settings: &AudioSettings, plugin_paths: &[PathBuf]) -> Self {
        let (device, engine) = auris_engine::start_silent(settings);
        Self::from_engine(device, engine, plugin_paths)
    }

    fn from_engine(device: AudioDevice, engine: EngineHandle, plugin_paths: &[PathBuf]) -> Self {
        let trust = crate::session::plugin_trust::PluginTrust::new(plugin_paths);
        let output_was_running = device.is_running();
        Self {
            device,
            engine,
            prepared: None,
            pending: None,
            published: None,
            retiring: Vec::new(),
            trust,
            parent: None,
            open_windows: BTreeSet::new(),
            output_was_running,
        }
    }

    /// Validates and prepares a revision without touching the published graph or its editors.
    /// The provider supplies policy/domain contracts; actual hosted descriptors and sidechain
    /// capabilities must agree. A failed replacement also preserves any previously prepared plan.
    pub fn prepare<P: CapabilityProvider>(
        &mut self,
        document: &NativeDocument,
        revision: u64,
        provider: &P,
        resources: NativeHostResources<'_>,
    ) -> Result<NativeGraphGeneration, NativeHostError> {
        self.prepare_with(document, revision, provider, resources, load_host)
    }

    fn prepare_with<P: CapabilityProvider>(
        &mut self,
        document: &NativeDocument,
        revision: u64,
        provider: &P,
        resources: NativeHostResources<'_>,
        mut load: impl FnMut(
            &Path,
            &auris_core::pipeline::content::PluginInstance,
            &PrepareContext,
            bool,
        ) -> Result<(Host, RenderHalf), NativeHostError>,
    ) -> Result<NativeGraphGeneration, NativeHostError> {
        if self.pending.is_some() {
            return Err(NativeHostError::Pending);
        }
        let input = prepare_native_conventional(
            document,
            provider,
            resources.content_limits,
            resources.graph_limits,
        )?;
        let prepare = PrepareContext::new(self.engine.sample_rate(), self.engine.max_block(), 2);
        let mut placement = Placement::default();
        let mut windows = BTreeMap::new();
        for processor in &document.content.processors {
            let (instance, instrument) = match &processor.kind {
                ProcessorKind::Instrument(plugin) => (plugin, true),
                ProcessorKind::Effect { plugin, .. } => (plugin, false),
                _ => continue,
            };
            let Some(file) = &instance.file else {
                continue;
            };
            let file = file
                .resolve_for_automatic_access(resources.project_folder)
                .filter(|path| self.trust.allows(path))
                .ok_or(NativeHostError::UntrustedPlugin(processor.id))?;
            let (host, half) = load(&file, instance, &prepare, instrument)
                .map_err(|error| error.at(processor.id, "load"))?;
            // Store both halves before any further failure so render ownership is released first.
            match half {
                RenderHalf::Effect(effect) => {
                    placement.effects.insert(processor.id, effect);
                }
                RenderHalf::Instrument(instrument) => {
                    placement.instruments.insert(processor.id, instrument);
                }
            }
            placement.hosts.insert(processor.id, host);
            let capabilities = provider
                .plugin(processor)
                .ok_or_else(|| NativeHostError::InvalidPlugin("missing host metadata".into()))?;
            let host = &placement.hosts[&processor.id];
            let (descriptors, sidechain) = match host {
                Host::Clap(host) => (host.parameters(), host.supports_sidechain()),
                Host::Vst3(host) => (host.parameters(), host.supports_sidechain()),
            };
            if !descriptors.iter().eq(capabilities
                .parameters
                .iter()
                .map(|parameter| &parameter.descriptor))
                || sidechain != capabilities.sidechain
            {
                return Err(NativeHostError::InvalidPlugin(
                    "host capabilities changed during preparation".into(),
                )
                .at(processor.id, "capabilities"));
            }
            let window = match &processor.kind {
                ProcessorKind::Effect { slot, .. } => PluginWindow::Effect(*slot),
                _ => {
                    let track = document
                        .content
                        .tracks
                        .iter()
                        .find(|track| {
                            document.content.groups.iter().any(|group| {
                                group.id == track.group
                                    && group.primary_instrument == Some(processor.id)
                            })
                        })
                        .ok_or_else(|| {
                            NativeHostError::InvalidPlugin(
                                "hosted instrument has no command binding".into(),
                            )
                        })?;
                    PluginWindow::Instrument(track.track)
                }
            };
            windows.insert(window, processor.id);
        }
        let graph = input.build(
            resources.bank,
            resources.registry,
            &mut placement.effects,
            &mut placement.instruments,
            self.engine.max_block(),
            self.engine.sample_rate(),
        )?;
        let generation = graph.native_generation().ok_or_else(|| {
            NativeHostError::InvalidPlugin("prepared graph has no native generation".into())
        })?;
        self.prepared = Some(Prepared {
            graph,
            owners: HostGeneration {
                generation,
                revision,
                hosts: placement.hosts,
                windows,
            },
        });
        Ok(generation)
    }

    /// Releases a candidate which has not been enqueued, leaving published playback untouched.
    pub fn discard_prepared(&mut self) {
        self.prepared = None;
    }

    /// Enqueues a candidate only for the caller's current document revision.
    /// A full queue or stale revision leaves the candidate available for retry or discard.
    pub fn queue_prepared(
        &mut self,
        current_revision: u64,
    ) -> Result<NativeGraphGeneration, NativeHostError> {
        if self.pending.is_some() {
            return Err(NativeHostError::Pending);
        }
        let prepared = self.prepared.as_ref().ok_or(NativeHostError::NotPrepared)?;
        if prepared.owners.revision != current_revision {
            return Err(NativeHostError::StaleRevision {
                prepared: prepared.owners.revision,
                current: current_revision,
            });
        }
        if !self.device.is_running() {
            return Err(EngineError::NotRunning.into());
        }
        let Prepared { graph, owners } =
            self.prepared.take().ok_or(NativeHostError::NotPrepared)?;
        match self.engine.try_set_graph(graph) {
            Ok(()) => {
                let generation = owners.generation;
                self.pending = Some(owners);
                Ok(generation)
            }
            Err((error, graph)) => {
                self.prepared = Some(Prepared {
                    graph: *graph,
                    owners,
                });
                Err(error.into())
            }
        }
    }

    /// Generation prepared on the control thread but not enqueued.
    pub fn prepared_generation(&self) -> Option<NativeGraphGeneration> {
        self.prepared.as_ref().map(|plan| plan.owners.generation)
    }
    /// Generation queued and waiting for callback adoption.
    pub fn pending_generation(&self) -> Option<NativeGraphGeneration> {
        self.pending.as_ref().map(|plan| plan.generation)
    }
    /// Generation whose host/editor bindings have been published after adoption.
    pub fn published_generation(&self) -> Option<NativeGraphGeneration> {
        self.published.as_ref().map(|plan| plan.generation)
    }
    /// Native document revision associated with the adopted host/editor bindings.
    pub fn published_revision(&self) -> Option<u64> {
        self.published.as_ref().map(|plan| plan.revision)
    }

    /// Services all live host instances, collects returned graphs, and applies adoption.
    pub fn poll(&mut self) -> NativeHostUpdate {
        let mut update = NativeHostUpdate::default();
        if let Some(plan) = &mut self.published {
            service_generation(plan, true, &mut update);
            for (node, host) in &plan.hosts {
                if !host.window_is_open() {
                    self.open_windows.remove(node);
                }
            }
        }
        if let Some(plan) = &mut self.pending {
            service_generation(plan, false, &mut update);
        }
        if let Some(plan) = &mut self.prepared {
            service_generation(&mut plan.owners, false, &mut update);
        }
        self.engine.collect_garbage();
        // Observe adoption before discarding a dead device's queue: it may have adopted just
        // before disconnecting. Adoption remains meaningful even without hardware playback.
        self.promote(self.engine.adopted_native_generation(), &mut update);
        let running = self.device.is_running();
        let lost_output = self.output_was_running && !running;
        self.output_was_running = running;
        if lost_output {
            update.errors.push(EngineError::NotRunning.into());
        }
        if !running && let Some(pending) = self.pending.take() {
            self.device.discard_pending();
            self.retiring.extend(pending.hosts.into_values());
            if !lost_output {
                update.errors.push(EngineError::NotRunning.into());
            }
        }
        for host in &mut self.retiring {
            host.service();
        }
        self.retiring.retain_mut(|host| !host.release());
        update
    }

    fn promote(&mut self, adopted: Option<NativeGraphGeneration>, update: &mut NativeHostUpdate) {
        if !self
            .pending
            .as_ref()
            .is_some_and(|plan| Some(plan.generation) == adopted)
        {
            return;
        }
        if let Some(old) = self.published.take() {
            for (node, mut host) in old.hosts {
                if let Err(error) = host.set_window_open(false, self.parent) {
                    update.errors.push(error.at(node, "close editor"));
                }
                self.retiring.push(host);
            }
        }
        self.published = self.pending.take();
        if let Some(plan) = &mut self.published {
            self.open_windows
                .retain(|node| plan.hosts.contains_key(node));
            for node in &self.open_windows {
                if let Some(host) = plan.hosts.get_mut(node)
                    && let Err(error) = host.set_window_open(true, self.parent)
                {
                    update.errors.push(error.at(*node, "open editor"));
                }
            }
            update.adopted = Some(plan.generation);
        }
    }

    /// Resolves the conventional editor subject through the adopted native binding.
    pub fn window_node(&self, window: PluginWindow) -> Option<NodeId> {
        self.published.as_ref()?.windows.get(&window).copied()
    }
    /// Whether the published host offers a native editor.
    pub fn has_window(&mut self, node: NodeId) -> bool {
        self.published
            .as_mut()
            .and_then(|plan| plan.hosts.get_mut(&node))
            .is_some_and(Host::has_window)
    }
    /// Whether the published instance's editor is currently open.
    pub fn window_is_open(&self, node: NodeId) -> bool {
        self.published
            .as_ref()
            .and_then(|plan| plan.hosts.get(&node))
            .is_some_and(Host::window_is_open)
    }
    /// Opens/closes the editor belonging to the adopted node, preserving its intent on handover.
    pub fn set_window_open(&mut self, node: NodeId, open: bool) -> Result<(), NativeHostError> {
        let host = self
            .published
            .as_mut()
            .and_then(|plan| plan.hosts.get_mut(&node))
            .ok_or(NativeHostError::NotPublished(node))?;
        host.set_window_open(open, self.parent)
            .map_err(|error| error.at(node, "editor"))?;
        if open {
            self.open_windows.insert(node);
        } else {
            self.open_windows.remove(&node);
        }
        Ok(())
    }

    /// Sets the native editor parent window.
    ///
    /// # Safety
    /// A supplied handle must name a live application window that outlives this playback owner.
    pub unsafe fn set_window_parent(&mut self, parent: Option<RawWindowHandle>) {
        self.parent = parent;
    }

    /// Captures the adopted instance's actual state without changing a document or history.
    pub fn snapshot(&mut self, node: NodeId) -> Result<NativeHostSnapshot, NativeHostError> {
        let plan = self
            .published
            .as_mut()
            .ok_or(NativeHostError::NotPublished(node))?;
        let host = plan
            .hosts
            .get_mut(&node)
            .ok_or(NativeHostError::NotPublished(node))?;
        let state = host
            .snapshot()
            .map_err(|error| error.at(node, "snapshot"))?;
        Ok(NativeHostSnapshot {
            node,
            revision: plan.revision,
            generation: plan.generation,
            state,
        })
    }

    /// Starts or stops transport on the owned native engine.
    pub fn set_playing(&self, playing: bool) -> Result<(), NativeHostError> {
        if !self.device.is_running() {
            return Err(EngineError::NotRunning.into());
        }
        self.engine.send(if playing {
            EngineCommand::Play
        } else {
            EngineCommand::Stop
        })?;
        Ok(())
    }

    /// Whether the owned output device can currently consume queued commands.
    pub fn is_running(&self) -> bool {
        self.device.is_running()
    }
}

fn service_generation(plan: &mut HostGeneration, published: bool, update: &mut NativeHostUpdate) {
    for (node, host) in &mut plan.hosts {
        if host.service() && published {
            update.changed.push((plan.generation, *node));
        }
        if let Host::Vst3(host) = host
            && let Some(error) = host.take_render_error()
        {
            update
                .errors
                .push(NativeHostError::Vst3(error).at(*node, "renderer"));
        }
    }
}

fn load_host(
    file: &Path,
    instance: &auris_core::pipeline::content::PluginInstance,
    prepare: &PrepareContext,
    instrument: bool,
) -> Result<(Host, RenderHalf), NativeHostError> {
    if instance.plugin_id.starts_with(auris_clap::ID_PREFIX) {
        clap::ClapHost::load(
            file,
            &instance.plugin_id,
            &instance.state,
            prepare,
            instrument,
        )
        .map(|(host, half)| (Host::Clap(host), half))
    } else if instance.plugin_id.starts_with(auris_vst3::ID_PREFIX) {
        vst3::Vst3Host::load(
            file,
            &instance.plugin_id,
            &instance.state,
            prepare,
            instrument,
        )
        .map(|(host, half)| (Host::Vst3(host), half))
    } else {
        Err(NativeHostError::InvalidPlugin(instance.plugin_id.clone()))
    }
}

#[cfg(test)]
mod external_tests;
#[cfg(test)]
mod tests;
