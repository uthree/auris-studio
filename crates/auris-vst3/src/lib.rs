//! Hosting third-party VST3 plugins for Auris Studio.
//!
//! The VST3 SDK has used the MIT license since version 3.8. This crate adapts the safe,
//! MIT-licensed `vst3-host` API to Auris' format-independent instrument and effect traits.

#![warn(missing_docs)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, ThreadId};

use auris_core::buffer::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{
    Effect, Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PluginKind,
    PrepareContext, ProcessContext,
};
use crossbeam_queue::ArrayQueue;
use thiserror::Error;
use vst3_host::{
    BusAudioBuffers, BusDirection, MediaType, MidiChannel, MidiEvent, Parameter, Plugin, Vst3Host,
};

const PARAMETER_POINTS_PER_ID: usize = 64;

/// Prefix used for VST3 class ids stored in Auris project files.
pub const ID_PREFIX: &str = "vst3:";

/// A reproducible VST3 preset address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vst3Preset {
    /// A program advertised by a unit's program list.
    Program {
        /// Unit identifier.
        unit: i32,
        /// Zero-based program index.
        index: i32,
    },
    /// A standard `.vstpreset` file.
    File(PathBuf),
}

/// An error reported while discovering, loading, or driving a VST3 plugin.
#[derive(Debug, Error)]
pub enum Vst3Error {
    /// The underlying host rejected the operation.
    #[error(transparent)]
    Host(#[from] vst3_host::Error),
    /// A plugin file did not expose the requested audio class.
    #[error("VST3 plugin `{0}` was not found in the selected bundle")]
    UnknownPlugin(String),
    /// The plugin instance is currently in use by another thread.
    #[error("VST3 plugin is busy")]
    Busy,
    /// A live renderer was stopped rather than continuing with incomplete input.
    #[error("VST3 renderer for `{plugin}` stopped: {failure}")]
    RendererStopped {
        /// Plugin display name.
        plugin: String,
        /// Why the renderer could not safely continue.
        failure: Vst3RenderFailure,
    },
    /// The control instance could not provide state required for a lossless save or rebuild.
    #[error("VST3 state for `{plugin}` could not be {operation}: {detail}")]
    StateSync {
        /// Plugin display name.
        plugin: String,
        /// Operation that was attempted.
        operation: &'static str,
        /// Underlying host diagnostic.
        detail: String,
    },
}

/// Why an audio-thread VST3 renderer stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vst3RenderFailure {
    /// A complete MIDI/automation batch did not fit the preallocated realtime queues.
    RealtimeCapacityExceeded,
    /// The plugin rejected preparation, transport, event delivery, or processing.
    ProcessingRejected,
}

impl std::fmt::Display for Vst3RenderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RealtimeCapacityExceeded => {
                f.write_str("its fixed realtime MIDI/automation capacity was exceeded")
            }
            Self::ProcessingRejected => f.write_str("the plugin rejected realtime processing"),
        }
    }
}

impl Vst3RenderFailure {
    const NONE: u32 = 0;
    const CAPACITY: u32 = 1;
    const PROCESSING: u32 = 2;

    fn encode(self) -> u32 {
        match self {
            Self::RealtimeCapacityExceeded => Self::CAPACITY,
            Self::ProcessingRejected => Self::PROCESSING,
        }
    }

    fn decode(value: u32) -> Option<Self> {
        match value {
            Self::CAPACITY => Some(Self::RealtimeCapacityExceeded),
            Self::PROCESSING => Some(Self::ProcessingRejected),
            _ => None,
        }
    }

    fn from_host(error: &vst3_host::Error) -> Self {
        if matches!(error, vst3_host::Error::RealtimeCapacityExceeded) {
            Self::RealtimeCapacityExceeded
        } else {
            Self::ProcessingRejected
        }
    }
}

/// Metadata for one audio class exported by a `.vst3` bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vst3PluginInfo {
    /// The plugin's stable, 32-hex-character VST3 class id.
    pub class_id: String,
    /// Display name.
    pub name: String,
    /// Manufacturer.
    pub vendor: String,
    /// Plugin version supplied by the bundle.
    pub version: String,
    /// Whether the class generates or transforms audio.
    pub kind: PluginKind,
    /// Browser group inferred from VST3 metadata.
    pub category: PluginCategory,
    /// Whether the plugin reports a native editor.
    pub has_gui: bool,
}

/// Source edits drained from a VST3 plugin's native editor.
#[derive(Debug, Default)]
pub struct Vst3SourceChanges {
    /// Current normalized parameter values changed by the editor.
    pub values: Vec<(ParamId, f32)>,
    /// Whether the plugin reported any source edit that makes the document dirty.
    pub source_changed: bool,
    /// Whether an opaque state, program, layout, or restart change requires a fresh renderer.
    pub renderer_reload_required: bool,
    /// Why a renderer stopped after a processing or bounded-capacity failure.
    pub renderer_failure: Option<Vst3RenderFailure>,
}

impl Vst3PluginInfo {
    /// The globally namespaced id written into an Auris project.
    pub fn auris_id(&self) -> String {
        format!("{ID_PREFIX}{}", self.class_id)
    }

    /// Converts the discovery record to Auris' common presentation type.
    pub fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: Cow::Owned(self.auris_id()),
            name: Cow::Owned(self.name.clone()),
            vendor: Cow::Owned(self.vendor.clone()),
            description: Cow::Owned(format!("VST3 {}", self.version)),
            kind: self.kind,
            category: self.category,
        }
    }
}

/// Lists the audio classes contained in one VST3 bundle.
///
/// Inspecting a VST3 bundle executes its code. Call this only for a file selected by the user;
/// bulk scanning should first use [`installed_vst3_files`], which only walks directories.
pub fn plugins_in(path: &Path) -> Result<Vec<Vst3PluginInfo>, Vst3Error> {
    let detailed = vst3_host::get_detailed_plugin_info(path)?;
    let fallback_kind = kind_of(&detailed.info.category, detailed.info.has_midi_input);
    let mut found: Vec<_> = detailed
        .classes
        .iter()
        .filter(|class| class.category.contains("Audio Module Class"))
        .map(|class| {
            let kind = kind_of(&detailed.info.category, detailed.info.has_midi_input);
            Vst3PluginInfo {
                class_id: class.class_id.clone(),
                name: if class.name.is_empty() {
                    detailed.info.name.clone()
                } else {
                    class.name.clone()
                },
                vendor: detailed.info.vendor.clone(),
                version: if class.version.is_empty() {
                    detailed.info.version.clone()
                } else {
                    class.version.clone()
                },
                kind,
                category: category_of(&detailed.info.category, kind),
                has_gui: detailed.info.has_gui,
            }
        })
        .collect();
    if found.is_empty() {
        found.push(Vst3PluginInfo {
            class_id: detailed.info.uid,
            name: detailed.info.name,
            vendor: detailed.info.vendor,
            version: detailed.info.version,
            kind: fallback_kind,
            category: category_of(&detailed.info.category, fallback_kind),
            has_gui: detailed.info.has_gui,
        });
    }
    Ok(found)
}

/// Returns installed `.vst3` bundles without loading plugin code.
pub fn installed_vst3_files(extra_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut host = match Vst3Host::new() {
        Ok(host) => host,
        Err(error) => {
            log::warn!("cannot create VST3 scanner: {error}");
            return Vec::new();
        }
    };
    for path in extra_paths {
        if let Err(error) = host.add_scan_path(path) {
            log::warn!("cannot add VST3 scan path `{}`: {error}", path.display());
        }
    }
    let mut files = host.scan_plugin_paths();
    files.sort();
    files.dedup();
    files
}

/// Returns the standard and caller-provided roots where VST3 bundles may be installed.
///
/// This performs no filesystem traversal. Callers that need stricter discovery budgets can walk
/// these roots themselves instead of using [`installed_vst3_files`].
pub fn vst3_search_paths(extra_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = vst3_host::discovery::scan_standard_paths();
    paths.extend(extra_paths.iter().cloned());
    paths.sort();
    paths.dedup();
    paths
}

fn load_configured_plugin(
    path: &Path,
    class_id: &str,
    prepare: &PrepareContext,
) -> Result<(Plugin, bool), Vst3Error> {
    let mut host = Vst3Host::builder()
        .sample_rate(prepare.sample_rate)
        .block_size(prepare.max_block_frames.max(1))
        .build()?;
    let mut plugin = host.load_plugin_class(path, class_id)?;
    let layout = plugin.audio_bus_layout()?;
    let sidechain = layout.inputs.len() > 1;
    for index in 1..layout.inputs.len() {
        plugin.set_bus_active(MediaType::Audio, BusDirection::Input, index as i32, true)?;
    }
    Ok((plugin, sidechain))
}

/// A loaded VST3 instance retained by the editing session.
///
/// The native editor and project-state calls use this control-thread instance. Render wrappers
/// own independent instances, so editor work can never stall or bypass an audio block.
pub struct Vst3Plugin {
    shared: Arc<Shared>,
    window: Option<vst3_host::PluginWindow>,
}

struct Shared {
    plugin: Arc<Mutex<Plugin>>,
    path: PathBuf,
    class_id: String,
    info: Vst3PluginInfo,
    descriptor: PluginDescriptor,
    parameters: Arc<Vec<ParamDescriptor>>,
    parameter_ids: Arc<Vec<u32>>,
    parameter_lookup: Arc<HashMap<u32, usize>>,
    snapshot: Arc<ParameterSnapshot>,
    active_renderers: Arc<AtomicUsize>,
    render_teardowns: Mutex<Vec<RenderTeardown>>,
    render_failure: Arc<AtomicU32>,
    full_refresh_required: AtomicBool,
    prepare: PrepareContext,
    sidechain: bool,
}

struct ParameterSnapshot {
    values: Box<[AtomicU32]>,
    revision: AtomicU64,
}

struct RenderTeardown {
    returned: Arc<ArrayQueue<ManuallyDrop<Plugin>>>,
    owner_thread: ThreadId,
}

impl ParameterSnapshot {
    fn new(values: &[f32]) -> Self {
        Self {
            values: values
                .iter()
                .map(|value| AtomicU32::new(value.to_bits()))
                .collect(),
            revision: AtomicU64::new(1),
        }
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    fn value(&self, index: usize) -> Option<f32> {
        self.values
            .get(index)
            .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
    }

    fn publish(&self, index: usize, value: f32) -> bool {
        let Some(slot) = self.values.get(index) else {
            return false;
        };
        if !value.is_finite() {
            return false;
        }
        let bits = value.to_bits();
        if slot.swap(bits, Ordering::Relaxed) == bits {
            return false;
        }
        self.revision.fetch_add(1, Ordering::Release);
        true
    }
}

/// Owns a render-thread value without ever destroying it on that thread.
struct DeferredOwnerDrop<T> {
    value: ManuallyDrop<T>,
    returned: Arc<ArrayQueue<ManuallyDrop<T>>>,
    active: Arc<AtomicUsize>,
}

impl<T> DeferredOwnerDrop<T> {
    fn new(
        value: T,
        returned: Arc<ArrayQueue<ManuallyDrop<T>>>,
        active: &Arc<AtomicUsize>,
    ) -> Self {
        active.fetch_add(1, Ordering::AcqRel);
        Self {
            value: ManuallyDrop::new(value),
            returned,
            active: Arc::clone(active),
        }
    }
}

impl<T> Deref for DeferredOwnerDrop<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> DerefMut for DeferredOwnerDrop<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

impl<T> Drop for DeferredOwnerDrop<T> {
    fn drop(&mut self) {
        // SAFETY: this is the only take of `value`; ManuallyDrop prevents a second destructor.
        let value = unsafe { ManuallyDrop::take(&mut self.value) };
        // A missing/full owner slot deliberately leaks the plugin instead of running COM teardown
        // on a possible audio thread. Dropping ManuallyDrop never drops its inner value.
        let _ = self.returned.push(ManuallyDrop::new(value));
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl std::fmt::Debug for Vst3Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vst3Plugin")
            .field("info", &self.shared.info)
            .finish_non_exhaustive()
    }
}

impl Vst3Plugin {
    /// Names and exact addresses from the plugin's VST3 program lists.
    pub fn presets(&self) -> Result<Vec<(String, Vst3Preset)>, Vst3Error> {
        Ok(self
            .lock()?
            .get_units()?
            .into_iter()
            .flat_map(|unit| {
                unit.programs
                    .into_iter()
                    .enumerate()
                    .map(move |(index, name)| {
                        (
                            name,
                            Vst3Preset::Program {
                                unit: unit.id,
                                index: index as i32,
                            },
                        )
                    })
            })
            .collect())
    }

    /// Loads a discovered program or standard preset on an isolated instance.
    pub fn load_preset(&self, preset: &Vst3Preset) -> Result<(), Vst3Error> {
        let mut plugin = self.lock()?;
        match preset {
            Vst3Preset::Program { unit, index } => plugin.select_program(*unit, *index)?,
            Vst3Preset::File(path) => plugin.load_vstpreset(path)?,
        }
        // Renderers own independent plugin instances and replay this lock-free parameter
        // snapshot after restoring opaque state. Publish the preset's values before a renderer
        // can be built, or its first block would restore the pre-preset values over the newly
        // selected program.
        let values =
            self.capture_all_parameter_values(&plugin, "published after loading preset")?;
        drop(plugin);
        for (param, value) in values {
            self.shared.snapshot.publish(param.index(), value);
        }
        Ok(())
    }
    /// Loads and configures one VST3 audio class.
    pub fn load(path: &Path, class_id: &str, prepare: &PrepareContext) -> Result<Self, Vst3Error> {
        let (plugin, sidechain) = load_configured_plugin(path, class_id, prepare)?;
        let raw_parameters = plugin.get_parameters()?;
        let (parameters, parameter_ids) = describe_parameters(&raw_parameters);
        let initial_values: Vec<f32> = parameter_ids
            .iter()
            .zip(&parameters)
            .map(|(id, descriptor)| {
                plugin
                    .get_parameter(*id)
                    .map(|value| descriptor.clamp(value as f32))
            })
            .collect::<Result<_, _>>()?;
        let parameter_lookup = parameter_ids
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect();
        let raw = plugin.info().clone();
        let kind = kind_of(&raw.category, raw.has_midi_input);
        let info = Vst3PluginInfo {
            class_id: raw.uid,
            name: raw.name,
            vendor: raw.vendor,
            version: raw.version,
            kind,
            category: category_of(&raw.category, kind),
            has_gui: raw.has_gui,
        };

        let render_class_id = info.class_id.clone();
        Ok(Self {
            shared: Arc::new(Shared {
                descriptor: info.descriptor(),
                info,
                plugin: Arc::new(Mutex::new(plugin)),
                path: path.to_path_buf(),
                class_id: render_class_id,
                parameters: Arc::new(parameters),
                parameter_ids: Arc::new(parameter_ids),
                parameter_lookup: Arc::new(parameter_lookup),
                snapshot: Arc::new(ParameterSnapshot::new(&initial_values)),
                active_renderers: Arc::new(AtomicUsize::new(0)),
                render_teardowns: Mutex::new(Vec::new()),
                render_failure: Arc::new(AtomicU32::new(Vst3RenderFailure::NONE)),
                full_refresh_required: AtomicBool::new(false),
                prepare: *prepare,
                sidechain,
            }),
            window: None,
        })
    }

    /// Discovery metadata for this instance.
    pub fn info(&self) -> &Vst3PluginInfo {
        &self.shared.info
    }
    /// Parameters in Auris runtime order.
    pub fn parameters(&self) -> &[ParamDescriptor] {
        &self.shared.parameters
    }
    /// Reads a normalized value from the plugin controller.
    pub fn value(&self, id: ParamId) -> Option<f32> {
        let vst_id = *self.shared.parameter_ids.get(id.index())?;
        self.lock()
            .ok()?
            .get_parameter(vst_id)
            .ok()
            .map(|v| v as f32)
    }
    /// Writes a normalized parameter value to the plugin controller and processor.
    pub fn set_param(&self, id: ParamId, value: f32) -> Result<(), Vst3Error> {
        let Some((&vst_id, descriptor)) = self
            .shared
            .parameter_ids
            .get(id.index())
            .zip(self.shared.parameters.get(id.index()))
        else {
            return Ok(());
        };
        let value = descriptor.clamp(value);
        self.lock()?.set_parameter(vst_id, value as f64)?;
        self.shared.snapshot.publish(id.index(), value);
        Ok(())
    }
    /// Serializes the plugin's opaque project state.
    pub fn save_state(&self) -> Result<Vec<u8>, Vst3Error> {
        Ok(self.lock()?.save_state()?)
    }

    /// Consumes native editor and preset changes and publishes their latest parameter values.
    ///
    /// The renderer reads the published snapshot without sharing the editor's mutex. A busy
    /// control instance is left alone, so its notifications remain queued for the next poll.
    pub fn take_source_changes(&self) -> Result<Vst3SourceChanges, Vst3Error> {
        self.service_teardowns();
        let mut plugin = match self.shared.plugin.try_lock() {
            Ok(plugin) => plugin,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Ok(Vst3SourceChanges {
                    renderer_failure: self.take_renderer_failure(),
                    ..Vst3SourceChanges::default()
                });
            }
            Err(std::sync::TryLockError::Poisoned(_)) => return Err(Vst3Error::Busy),
        };
        let mut changes = self.take_source_changes_locked(&mut plugin, None)?;
        changes.renderer_failure = self.take_renderer_failure();
        Ok(changes)
    }

    /// Consumes editor changes while waiting for the control instance when saving.
    ///
    /// Audio rendering owns another instance and cannot be stalled by this control-thread wait.
    /// The blocking form lets a save persist the opaque state and matching normalized values as
    /// one ordered snapshot instead of allowing a busy editor poll to leave stale parameters.
    pub fn take_source_changes_for_save(&self) -> Result<(Vst3SourceChanges, Vec<u8>), Vst3Error> {
        self.service_teardowns();
        let mut plugin = self.lock()?;
        // Capture first: if serialization fails, notifications remain queued and a later poll or
        // save can retry without having lost the corresponding document parameter updates.
        let state = plugin.save_state().map_err(|source| Vst3Error::StateSync {
            plugin: self.shared.info.name.clone(),
            operation: "captured for saving",
            detail: source.to_string(),
        })?;
        // Project parameters are restored after the opaque state, so every value must come from
        // the same live control instance. Otherwise a stale document value would overwrite the
        // newer value embedded in `state` when the project is opened again.
        let values = self.capture_all_parameter_values(&plugin, "synchronized for saving")?;
        let mut changes = self.take_source_changes_locked(&mut plugin, Some(values))?;
        changes.renderer_failure = self.take_renderer_failure();
        Ok((changes, state))
    }

    fn take_renderer_failure(&self) -> Option<Vst3RenderFailure> {
        Vst3RenderFailure::decode(
            self.shared
                .render_failure
                .swap(Vst3RenderFailure::NONE, Ordering::AcqRel),
        )
    }

    fn take_source_changes_locked(
        &self,
        plugin: &mut Plugin,
        captured_values: Option<Vec<(ParamId, f32)>>,
    ) -> Result<Vst3SourceChanges, Vst3Error> {
        let edits = plugin.take_parameter_edits();
        let notifications = plugin.take_host_notifications();
        let restart = plugin.take_restart_flags();
        let dirty = notifications.iter().any(|notification| {
            matches!(
                notification,
                vst3_host::HostNotification::DirtyChanged(true)
            )
        });
        let program_changed = notifications.iter().any(|notification| {
            matches!(
                notification,
                vst3_host::HostNotification::ProgramListChanged { .. }
            )
        });
        let retry_refresh = self.shared.full_refresh_required.load(Ordering::Acquire);
        let opaque_changed = retry_refresh || dirty || program_changed || !restart.is_empty();
        let mut parameter_changed = false;
        let values = if let Some(values) = captured_values {
            for &(param, value) in &values {
                parameter_changed |= self.shared.snapshot.publish(param.index(), value);
            }
            self.shared
                .full_refresh_required
                .store(false, Ordering::Release);
            values
        } else if opaque_changed {
            let values = match self.capture_all_parameter_values(plugin, "synchronized") {
                Ok(values) => values,
                Err(error) => {
                    self.shared
                        .full_refresh_required
                        .store(true, Ordering::Release);
                    return Err(error);
                }
            };
            for &(param, value) in &values {
                parameter_changed |= self.shared.snapshot.publish(param.index(), value);
            }
            self.shared
                .full_refresh_required
                .store(false, Ordering::Release);
            values
        } else {
            let mut values = Vec::new();
            for edit in edits {
                if !matches!(edit.kind, vst3_host::ParameterEditKind::ValueChange) {
                    continue;
                }
                let (Some(value), Some(&index)) =
                    (edit.value, self.shared.parameter_lookup.get(&edit.id))
                else {
                    continue;
                };
                let value = value as f32;
                if !value.is_finite() {
                    // `take_parameter_edits` drained the complete gesture batch. Force a full
                    // controller snapshot on the next poll so valid edits that preceded this
                    // malformed value cannot be lost merely because this pass is rejected.
                    self.shared
                        .full_refresh_required
                        .store(true, Ordering::Release);
                    return Err(self.state_sync_error(
                        "synchronized",
                        "the editor reported a non-finite parameter value".into(),
                    ));
                }
                let value = self.shared.parameters[index].clamp(value);
                values.push((ParamId(index as u32), value));
            }
            // Validate the complete drained batch before publishing any part of it. Once all
            // values are known-good, retain only actual changes while preserving edit order.
            values.retain(|(param, value)| self.shared.snapshot.publish(param.index(), *value));
            parameter_changed = !values.is_empty();
            values
        };
        let source_changed = parameter_changed || opaque_changed;
        Ok(Vst3SourceChanges {
            values,
            source_changed,
            renderer_reload_required: opaque_changed,
            renderer_failure: None,
        })
    }

    fn capture_all_parameter_values(
        &self,
        plugin: &Plugin,
        operation: &'static str,
    ) -> Result<Vec<(ParamId, f32)>, Vst3Error> {
        let mut values = Vec::with_capacity(self.shared.parameter_ids.len());
        for (index, raw_id) in self.shared.parameter_ids.iter().enumerate() {
            let value = plugin
                .get_parameter(*raw_id)
                .map_err(|source| self.state_sync_error(operation, source.to_string()))?
                as f32;
            if !value.is_finite() {
                return Err(self.state_sync_error(
                    operation,
                    format!("parameter {raw_id} returned a non-finite value"),
                ));
            }
            values.push((
                ParamId(index as u32),
                self.shared.parameters[index].clamp(value),
            ));
        }
        Ok(values)
    }

    fn state_sync_error(&self, operation: &'static str, detail: String) -> Vst3Error {
        Vst3Error::StateSync {
            plugin: self.shared.info.name.clone(),
            operation,
            detail,
        }
    }

    /// Consumes native editor notifications and reports whether the source changed.
    ///
    /// Prefer [`Self::take_source_changes`] when the caller must persist parameter values.
    pub fn take_source_changed(&self) -> Result<bool, Vst3Error> {
        Ok(self.take_source_changes()?.source_changed)
    }
    /// Restores an opaque state blob previously produced by this class.
    pub fn load_state(&self, bytes: &[u8]) -> Result<(), Vst3Error> {
        let mut plugin = self.lock()?;
        plugin.load_state(bytes)?;
        let values = self.capture_all_parameter_values(&plugin, "published after restoring")?;
        drop(plugin);
        for (param, value) in values {
            self.shared.snapshot.publish(param.index(), value);
        }
        Ok(())
    }
    /// Creates an effect wrapper for the render graph.
    pub fn effect(&self) -> Result<Vst3Effect, Vst3Error> {
        self.service_teardowns();
        Vst3Effect::new(&self.shared)
    }
    /// Creates an instrument wrapper for the render graph.
    pub fn instrument(&self) -> Result<Vst3Instrument, Vst3Error> {
        self.service_teardowns();
        Vst3Instrument::new(&self.shared)
    }
    /// Whether the plugin has a secondary input bus.
    pub fn wants_sidechain(&self) -> bool {
        self.shared.sidechain
    }
    /// Whether the plugin advertises a native editor.
    pub fn has_gui(&self) -> bool {
        self.shared.info.has_gui
    }
    /// Whether this instance's native editor is currently visible.
    pub fn gui_is_open(&self) -> bool {
        self.window
            .as_ref()
            .is_some_and(vst3_host::PluginWindow::is_open)
    }
    /// Opens or closes the plugin's standalone native editor.
    pub fn set_gui_open(&mut self, open: bool) -> Result<(), Vst3Error> {
        if open {
            if self.gui_is_open() {
                return Ok(());
            }
            let mut window = vst3_host::PluginWindow::new(Arc::clone(&self.shared.plugin));
            window.open()?;
            self.window = Some(window);
        } else if let Some(mut window) = self.window.take() {
            window.close();
        }
        Ok(())
    }
    /// Whether no render-graph wrapper still refers to this instance.
    pub fn is_idle(&self) -> bool {
        self.service_teardowns();
        self.shared.active_renderers.load(Ordering::Acquire) == 0
    }

    fn service_teardowns(&self) {
        let Ok(mut teardowns) = self.shared.render_teardowns.lock() else {
            return;
        };
        let current = thread::current().id();
        teardowns.retain_mut(|teardown| {
            if current != teardown.owner_thread {
                return true;
            }
            match teardown.returned.pop() {
                Some(plugin) => {
                    drop(ManuallyDrop::into_inner(plugin));
                    false
                }
                None => true,
            }
        });
    }

    fn lock(&self) -> Result<MutexGuard<'_, Plugin>, Vst3Error> {
        self.shared.plugin.lock().map_err(|_| Vst3Error::Busy)
    }
}

impl Drop for Vst3Plugin {
    fn drop(&mut self) {
        self.service_teardowns();
    }
}

/// A VST3 audio effect as seen by the render graph.
pub struct Vst3Effect(Bridge);
impl std::fmt::Debug for Vst3Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Vst3Effect")
            .field(&self.0.descriptor.name)
            .finish()
    }
}
impl Vst3Effect {
    /// Whether parameter delivery or rendering failed for this instance.
    pub fn processing_failed(&self) -> bool {
        self.0.processing_failed
    }

    fn new(shared: &Arc<Shared>) -> Result<Self, Vst3Error> {
        Ok(Self(Bridge::new(shared)?))
    }
}

/// A VST3 software instrument as seen by the render graph.
pub struct Vst3Instrument(Bridge);
impl std::fmt::Debug for Vst3Instrument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Vst3Instrument")
            .field(&self.0.descriptor.name)
            .finish()
    }
}
impl Vst3Instrument {
    /// Whether rendering, MIDI delivery or access to the processor failed for this instance.
    /// Offline measurement must distinguish failed processing from a silent sound.
    pub fn processing_failed(&self) -> bool {
        self.0.processing_failed
    }

    fn new(shared: &Arc<Shared>) -> Result<Self, Vst3Error> {
        Ok(Self(Bridge::new(shared)?))
    }
}

struct Bridge {
    plugin: DeferredOwnerDrop<Plugin>,
    buffers: BusAudioBuffers,
    descriptor: PluginDescriptor,
    parameters: Arc<Vec<ParamDescriptor>>,
    parameter_ids: Arc<Vec<u32>>,
    midi_parameter_indices: [Option<usize>; 130],
    parameter_queue_capacity: usize,
    snapshot: Arc<ParameterSnapshot>,
    snapshot_revision: u64,
    render_failure: Arc<AtomicU32>,
    prepare: PrepareContext,
    sidechain: bool,
    values: Vec<f32>,
    dirty_parameters: Vec<bool>,
    parameter_point_counts: Vec<u8>,
    latency: usize,
    prepared: bool,
    fatal_failure: bool,
    processing_failed: bool,
}

impl Bridge {
    fn new(shared: &Arc<Shared>) -> Result<Self, Vst3Error> {
        let state = shared
            .plugin
            .lock()
            .map_err(|_| Vst3Error::Busy)?
            .save_state()?;
        let (mut plugin, _) =
            load_configured_plugin(&shared.path, &shared.class_id, &shared.prepare)?;
        plugin.load_state(&state)?;
        // Capture the revision before its values. A concurrent publication after this load will
        // then leave a larger revision for the first render block to observe; reading the
        // revision afterwards could incorrectly declare a partly old snapshot up to date.
        let snapshot_revision = shared.snapshot.revision();
        let values: Vec<f32> = (0..shared.parameters.len())
            .map(|index| {
                shared
                    .snapshot
                    .value(index)
                    .unwrap_or(shared.parameters[index].default)
            })
            .collect();
        let dirty_parameters: Vec<bool> = shared
            .parameter_ids
            .iter()
            .zip(shared.parameters.iter())
            .zip(&values)
            .map(|((raw_id, descriptor), value)| {
                plugin
                    .get_parameter(*raw_id)
                    .map(|current| descriptor.clamp(current as f32).to_bits() != value.to_bits())
            })
            .collect::<Result<_, _>>()?;
        let midi_parameter_indices = std::array::from_fn(|controller| {
            plugin
                .midi_cc_to_parameter(0, 0, controller as u16)
                .map(|raw_id| {
                    shared
                        .parameter_lookup
                        .get(&raw_id)
                        .copied()
                        .unwrap_or(usize::MAX)
                })
        });
        let mut reset_unknown_parameter_points = 0_usize;
        for channel in 0..16_i16 {
            for controller in [123_u16, 120, 121] {
                let Some(raw_id) = plugin.midi_cc_to_parameter(0, channel, controller) else {
                    continue;
                };
                if !shared.parameter_lookup.contains_key(&raw_id) {
                    reset_unknown_parameter_points += 1;
                }
            }
        }
        let buffers = plugin.create_bus_audio_buffers(shared.prepare.max_block_frames.max(1))?;
        let unknown_mapping_slots = midi_parameter_indices
            .iter()
            .filter(|index| **index == Some(usize::MAX))
            .count();
        let parameter_queue_capacity = shared
            .parameters
            .len()
            .saturating_add(unknown_mapping_slots)
            .saturating_add(reset_unknown_parameter_points)
            .min(vst3_host::plugin::REALTIME_PARAMETER_CAPACITY);
        let latency = plugin.latency_samples() as usize;
        let returned = Arc::new(ArrayQueue::new(1));
        shared
            .render_teardowns
            .lock()
            .map_err(|_| Vst3Error::Busy)?
            .push(RenderTeardown {
                returned: Arc::clone(&returned),
                owner_thread: thread::current().id(),
            });
        let plugin = DeferredOwnerDrop::new(plugin, returned, &shared.active_renderers);
        Ok(Self {
            plugin,
            buffers,
            descriptor: shared.descriptor.clone(),
            parameters: Arc::clone(&shared.parameters),
            parameter_ids: Arc::clone(&shared.parameter_ids),
            midi_parameter_indices,
            parameter_queue_capacity,
            snapshot: Arc::clone(&shared.snapshot),
            snapshot_revision,
            render_failure: Arc::clone(&shared.render_failure),
            prepare: shared.prepare,
            sidechain: shared.sidechain,
            parameter_point_counts: vec![0; values.len()],
            values,
            dirty_parameters,
            latency,
            prepared: false,
            fatal_failure: false,
            processing_failed: false,
        })
    }

    fn prepare(&mut self) {
        if self.prepared || self.fatal_failure {
            return;
        }
        if let Err(error) = self
            .plugin
            .prepare_realtime_bus_processing(self.parameter_queue_capacity, PARAMETER_POINTS_PER_ID)
            .and_then(|()| self.plugin.start_processing())
        {
            self.fail(Vst3RenderFailure::from_host(&error));
            return;
        }
        self.prepared = true;
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if !value.is_finite() {
            self.fail(Vst3RenderFailure::ProcessingRejected);
            return;
        }
        let Some(descriptor) = self.parameters.get(id.index()) else {
            return;
        };
        let value = descriptor.clamp(value);
        if self.values[id.index()].to_bits() != value.to_bits() {
            self.values[id.index()] = value;
            self.dirty_parameters[id.index()] = true;
        }
    }

    fn sync_control_parameters(&mut self) {
        let revision = self.snapshot.revision();
        if revision == self.snapshot_revision {
            return;
        }
        for index in 0..self.values.len() {
            let Some(value) = self.snapshot.value(index) else {
                continue;
            };
            if value.to_bits() == self.values[index].to_bits() {
                continue;
            }
            self.values[index] = value;
            self.dirty_parameters[index] = true;
        }
        self.snapshot_revision = revision;
    }

    fn reset_voices(&mut self) {
        if !self.prepared || self.fatal_failure {
            return;
        }
        // The vendored host preflights the exact number of tracked voices (including duplicate
        // note-ons for one pitch) and mapped panic parameters before appending the first item.
        if let Err(error) = self.plugin.midi_panic() {
            self.fail(Vst3RenderFailure::from_host(&error));
            return;
        }
        // Flush releases on their own zero-sample process call. The next render block can then
        // chase notes at a seek destination without sharing the fixed event queue with panic.
        self.buffers.block_size = 0;
        self.buffers.clear();
        if let Err(error) = self.plugin.process_realtime_bus_audio(&mut self.buffers) {
            self.fail(Vst3RenderFailure::from_host(&error));
        }
    }

    fn reset_processing_state(&mut self) {
        if !self.prepared || self.fatal_failure {
            return;
        }
        // VST3 explicitly permits setProcessing on the processing thread and requires a plugin
        // to clear delay/reverb and other DSP history when processing starts again. The vendored
        // host performs only the false/true notification pair and fixed-capacity queue cleanup;
        // component activation remains on the control thread.
        if let Err(error) = self.plugin.reset_realtime_processing() {
            self.fail(Vst3RenderFailure::from_host(&error));
            return;
        }
        self.buffers.clear();
    }

    fn parameter_index_for_event(&self, event: NoteEvent) -> Option<usize> {
        let controller = match event {
            NoteEvent::PitchBend { .. } => 129,
            NoteEvent::Controller { number, .. } => usize::from(number),
            NoteEvent::NoteOn { .. }
            | NoteEvent::NoteOff { .. }
            | NoteEvent::AllNotesOff { .. }
            | NoteEvent::AllSoundOff { .. } => return None,
        };
        self.midi_parameter_indices
            .get(controller)
            .copied()
            .flatten()
    }

    fn batch_fits(&mut self, events: &[NoteEvent]) -> bool {
        self.parameter_point_counts.fill(0);
        let mut parameter_points = 0_usize;
        for (index, dirty) in self.dirty_parameters.iter().copied().enumerate() {
            if dirty {
                self.parameter_point_counts[index] = 1;
                parameter_points += 1;
            }
        }
        let event_points = events
            .iter()
            .filter(|event| matches!(event, NoteEvent::NoteOn { .. } | NoteEvent::NoteOff { .. }))
            .count();
        let new_voices = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    NoteEvent::NoteOn { velocity, .. } if midi_velocity(*velocity) != 0
                )
            })
            .count();
        let mut unknown_parameter_points = 0_usize;
        for &event in events {
            let Some(index) = self.parameter_index_for_event(event) else {
                continue;
            };
            parameter_points += 1;
            if index == usize::MAX {
                unknown_parameter_points += 1;
                continue;
            }
            self.parameter_point_counts[index] =
                self.parameter_point_counts[index].saturating_add(1);
        }
        new_voices <= self.plugin.realtime_note_capacity_remaining()
            && realtime_batch_fits(
                event_points,
                parameter_points,
                unknown_parameter_points,
                &self.parameter_point_counts,
            )
    }

    fn apply_parameters(&mut self) -> Result<(), vst3_host::Error> {
        for index in 0..self.dirty_parameters.len() {
            if !self.dirty_parameters[index] {
                continue;
            }
            self.plugin.set_parameter_at(
                self.parameter_ids[index],
                self.values[index] as f64,
                0,
            )?;
            self.dirty_parameters[index] = false;
        }
        Ok(())
    }

    fn fail(&mut self, failure: Vst3RenderFailure) {
        self.processing_failed = true;
        self.fatal_failure = true;
        let _ = self.render_failure.compare_exchange(
            Vst3RenderFailure::NONE,
            failure.encode(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn render(
        &mut self,
        buffer: &mut AudioBuffer,
        events: &[NoteEvent],
        overwrite: bool,
        sidechain: Option<&AudioBuffer>,
        ctx: &ProcessContext,
    ) {
        let frames = buffer.frame_count();
        if frames == 0 {
            return;
        }
        if !self.prepared || self.fatal_failure {
            if overwrite {
                buffer.clear();
            }
            return;
        }
        self.sync_control_parameters();
        if !process_context_is_valid(ctx, frames, &self.prepare)
            || !events
                .iter()
                .all(|event| note_event_is_valid(event, frames))
        {
            // Validate the whole block before queueing the first item. A malformed context or
            // late event must not leave earlier parameters/note-ons delivered without their
            // corresponding releases.
            self.reset_voices();
            if !self.fatal_failure {
                self.fail(Vst3RenderFailure::ProcessingRejected);
            }
            if overwrite {
                buffer.clear();
            }
            return;
        }
        if events.iter().any(|event| {
            matches!(
                event,
                NoteEvent::AllNotesOff { .. } | NoteEvent::AllSoundOff { .. }
            )
        }) {
            // VST3 has no generic input-CC event. IMidiMapping is optional, so translating a
            // discontinuity to CC 123/120 can be a successful no-op. Auris emits these panic
            // markers at block offset zero; flush the exact host-tracked voices instead.
            self.reset_voices();
            if self.fatal_failure {
                if overwrite {
                    buffer.clear();
                }
                return;
            }
        }
        if !self.batch_fits(events) {
            // The rejected batch has not touched either realtime queue. Release voices left by
            // the preceding successful block before stopping this renderer, so a queue-capacity
            // failure cannot leave a synth voice alive until eventual graph teardown.
            self.reset_voices();
            if !self.fatal_failure {
                self.fail(Vst3RenderFailure::RealtimeCapacityExceeded);
            }
            if overwrite {
                buffer.clear();
            }
            return;
        }
        let control_result = self
            .apply_parameters()
            .and_then(|()| self.plugin.set_tempo(ctx.bpm))
            .and_then(|()| self.plugin.set_playing(ctx.is_playing));
        if let Err(error) = control_result {
            self.fail(Vst3RenderFailure::from_host(&error));
            if overwrite {
                buffer.clear();
            }
            return;
        }
        for event in events {
            if matches!(
                event,
                NoteEvent::AllNotesOff { .. } | NoteEvent::AllSoundOff { .. }
            ) {
                continue;
            }
            let midi = translate_event(*event);
            if let Err(error) = self.plugin.send_midi_event_at(midi, event.frame() as i32) {
                self.fail(Vst3RenderFailure::from_host(&error));
                if overwrite {
                    buffer.clear();
                }
                return;
            }
        }
        self.buffers.block_size = frames;
        self.buffers.sample_rate = ctx.sample_rate;
        self.buffers.clear();
        if let Some(main) = self.buffers.inputs.first_mut() {
            copy_into_bus(main.channels.as_mut_slice(), Some(buffer), frames);
        }
        if let Some(key) = self.buffers.inputs.get_mut(1) {
            copy_into_bus(key.channels.as_mut_slice(), sidechain, frames);
        }
        if let Err(error) = self.plugin.process_realtime_bus_audio(&mut self.buffers) {
            self.fail(Vst3RenderFailure::from_host(&error));
            if overwrite {
                buffer.clear();
            }
            return;
        }
        let Some(main) = self.buffers.outputs.iter().find(|bus| bus.active) else {
            self.fail(Vst3RenderFailure::ProcessingRejected);
            if overwrite {
                buffer.clear();
            }
            return;
        };
        deliver(main.channels.as_slice(), buffer, frames, overwrite);
    }
}

fn realtime_batch_fits(
    events: usize,
    parameter_points: usize,
    unknown_parameter_points: usize,
    points_per_parameter: &[u8],
) -> bool {
    events <= vst3_host::plugin::REALTIME_EVENT_CAPACITY
        && parameter_points <= vst3_host::plugin::REALTIME_PARAMETER_CAPACITY
        && unknown_parameter_points <= PARAMETER_POINTS_PER_ID
        && points_per_parameter
            .iter()
            .all(|&count| usize::from(count) <= PARAMETER_POINTS_PER_ID)
}

fn process_context_is_valid(
    context: &ProcessContext,
    buffer_frames: usize,
    prepare: &PrepareContext,
) -> bool {
    context.sample_rate.is_finite()
        && context.sample_rate > 0.0
        && context.sample_rate.to_bits() == prepare.sample_rate.to_bits()
        && context.bpm.is_finite()
        && context.bpm > 0.0
        && context.block_frames == buffer_frames
        && buffer_frames <= prepare.max_block_frames
}

impl Parameterized for Vst3Effect {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.0.parameters
    }
    fn param(&self, id: ParamId) -> f32 {
        self.0.values.get(id.index()).copied().unwrap_or(0.0)
    }
    fn set_param(&mut self, id: ParamId, value: f32) {
        self.0.set_param(id, value);
    }
}
impl Effect for Vst3Effect {
    fn descriptor(&self) -> PluginDescriptor {
        self.0.descriptor.clone()
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.0.prepare();
    }
    fn reset(&mut self) {
        self.0.reset_processing_state();
    }
    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        self.0.render(buffer, &[], false, None, ctx);
    }
    fn wants_sidechain(&self) -> bool {
        self.0.sidechain
    }
    fn process_with_sidechain(
        &mut self,
        buffer: &mut AudioBuffer,
        sidechain: &AudioBuffer,
        ctx: &ProcessContext,
    ) {
        self.0.render(buffer, &[], false, Some(sidechain), ctx);
    }
    fn latency_frames(&self) -> usize {
        self.0.latency
    }
}
impl Parameterized for Vst3Instrument {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.0.parameters
    }
    fn param(&self, id: ParamId) -> f32 {
        self.0.values.get(id.index()).copied().unwrap_or(0.0)
    }
    fn set_param(&mut self, id: ParamId, value: f32) {
        self.0.set_param(id, value);
    }
}
impl Instrument for Vst3Instrument {
    fn descriptor(&self) -> PluginDescriptor {
        self.0.descriptor.clone()
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.0.prepare();
    }
    fn reset(&mut self) {
        self.0.reset_voices();
    }
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        self.0.render(out, events, true, None, ctx);
    }
}

fn describe_parameters(raw: &[Parameter]) -> (Vec<ParamDescriptor>, Vec<u32>) {
    let mut descriptions = Vec::with_capacity(raw.len());
    let mut ids = Vec::with_capacity(raw.len());
    for parameter in raw {
        let index = descriptions.len() as u32;
        let mut descriptor = ParamDescriptor {
            id: ParamId(index),
            key: Cow::Owned(format!("vst3.{}", parameter.id)),
            name: Cow::Owned(parameter.name.clone()),
            min: 0.0,
            max: 1.0,
            default: parameter.default.clamp(0.0, 1.0) as f32,
            unit: unit_of(parameter),
            curve: auris_core::param::ParamValueCurve::Linear,
            steps: (parameter.step_count > 0).then_some(parameter.step_count as u32 + 1),
            choices: Cow::Owned(Vec::new()),
        };
        if parameter.is_boolean() {
            descriptor.unit = ParamUnit::Toggle;
        }
        descriptions.push(descriptor);
        ids.push(parameter.id);
    }
    (descriptions, ids)
}

fn unit_of(parameter: &Parameter) -> ParamUnit {
    if parameter.is_boolean() {
        return ParamUnit::Toggle;
    }
    match parameter.unit.trim().to_ascii_lowercase().as_str() {
        "db" => ParamUnit::Decibels,
        "hz" | "khz" => ParamUnit::Hertz,
        "s" | "sec" => ParamUnit::Seconds,
        "ms" => ParamUnit::Milliseconds,
        "%" => ParamUnit::Percent,
        "bpm" => ParamUnit::Bpm,
        _ => ParamUnit::Plain,
    }
}

fn kind_of(category: &str, midi_input: bool) -> PluginKind {
    let lower = category.to_ascii_lowercase();
    if lower.contains("instrument")
        || lower.contains("synth")
        || (midi_input && !lower.contains("fx"))
    {
        PluginKind::Instrument
    } else {
        PluginKind::Effect
    }
}

fn category_of(category: &str, kind: PluginKind) -> PluginCategory {
    let value = category.to_ascii_lowercase();
    for (needle, category) in [
        ("reverb", PluginCategory::Reverb),
        ("delay", PluginCategory::Delay),
        ("dynamics", PluginCategory::Dynamics),
        ("compress", PluginCategory::Dynamics),
        ("eq", PluginCategory::Equalizer),
        ("filter", PluginCategory::Equalizer),
        ("distortion", PluginCategory::Distortion),
        ("modulation", PluginCategory::Modulation),
        ("analyzer", PluginCategory::Utility),
        ("drum", PluginCategory::Drum),
        ("sampler", PluginCategory::Sampler),
        ("synth", PluginCategory::Synth),
    ] {
        if value.contains(needle) {
            return category;
        }
    }
    if kind == PluginKind::Instrument {
        PluginCategory::Synth
    } else {
        PluginCategory::Other
    }
}

fn translate_event(event: NoteEvent) -> MidiEvent {
    let channel = MidiChannel::Ch1;
    match event {
        NoteEvent::NoteOn {
            pitch, velocity, ..
        } => MidiEvent::NoteOn {
            channel,
            note: pitch,
            velocity: midi_velocity(velocity),
        },
        NoteEvent::NoteOff { pitch, .. } => MidiEvent::NoteOff {
            channel,
            note: pitch,
            velocity: 0,
        },
        NoteEvent::AllNotesOff { .. } => MidiEvent::ControlChange {
            channel,
            controller: 123,
            value: 0,
        },
        NoteEvent::AllSoundOff { .. } => MidiEvent::ControlChange {
            channel,
            controller: 120,
            value: 0,
        },
        NoteEvent::PitchBend { semitones, .. } => MidiEvent::PitchBend {
            channel,
            value: (8192.0 + semitones.clamp(-2.0, 2.0) * 4096.0)
                .round()
                .clamp(0.0, 16383.0) as u16,
        },
        NoteEvent::Controller { number, value, .. } => MidiEvent::ControlChange {
            channel,
            controller: number,
            value: (value.clamp(0.0, 1.0) * 127.0).round() as u8,
        },
    }
}

fn midi_velocity(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 127.0).round() as u8
}

fn note_event_is_valid(event: &NoteEvent, frames: usize) -> bool {
    if event.frame() as usize >= frames {
        return false;
    }
    match *event {
        NoteEvent::NoteOn {
            pitch, velocity, ..
        } => pitch <= 127 && velocity.is_finite(),
        NoteEvent::NoteOff { pitch, .. } => pitch <= 127,
        NoteEvent::Controller { number, value, .. } => number <= 127 && value.is_finite(),
        NoteEvent::AllNotesOff { frame } | NoteEvent::AllSoundOff { frame } => frame == 0,
        NoteEvent::PitchBend { semitones, .. } => semitones.is_finite(),
    }
}

fn copy_into_bus(channels: &mut [Vec<f32>], source: Option<&AudioBuffer>, frames: usize) {
    for (index, channel) in channels.iter_mut().enumerate() {
        let copied = source.and_then(|b| b.try_channel(index)).map_or(0, |src| {
            let count = frames.min(src.len()).min(channel.len());
            channel[..count].copy_from_slice(&src[..count]);
            count
        });
        let end = frames.min(channel.len());
        channel[copied.min(end)..end].fill(0.0);
    }
}

fn deliver(channels: &[Vec<f32>], target: &mut AudioBuffer, frames: usize, overwrite: bool) {
    for (index, source) in channels.iter().enumerate().take(target.channel_count()) {
        let count = frames.min(source.len());
        target.channel_mut(index)[..count].copy_from_slice(&source[..count]);
    }
    if overwrite {
        if channels.is_empty() {
            target.clear();
            return;
        }
        for index in channels.len()..target.channel_count() {
            let source = &channels[index % channels.len()];
            let count = frames.min(source.len());
            target.channel_mut(index)[..count].copy_from_slice(&source[..count]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn ids_are_namespaced() {
        let info = Vst3PluginInfo {
            class_id: "00112233445566778899aabbccddeeff".into(),
            name: "Test".into(),
            vendor: String::new(),
            version: String::new(),
            kind: PluginKind::Effect,
            category: PluginCategory::Other,
            has_gui: false,
        };
        assert_eq!(info.auris_id(), "vst3:00112233445566778899aabbccddeeff");
    }
    #[test]
    fn vst_categories_are_classified_conservatively() {
        assert_eq!(kind_of("Instrument|Synth", true), PluginKind::Instrument);
        assert_eq!(kind_of("Fx|Reverb", false), PluginKind::Effect);
        assert_eq!(
            category_of("Fx|Reverb", PluginKind::Effect),
            PluginCategory::Reverb
        );
    }

    #[test]
    fn oversized_realtime_batches_are_rejected_before_partial_delivery() {
        let event_limit = vst3_host::plugin::REALTIME_EVENT_CAPACITY;
        let parameter_limit = vst3_host::plugin::REALTIME_PARAMETER_CAPACITY;

        assert!(realtime_batch_fits(
            event_limit,
            parameter_limit,
            PARAMETER_POINTS_PER_ID,
            &[PARAMETER_POINTS_PER_ID as u8],
        ));
        assert!(!realtime_batch_fits(event_limit + 1, 0, 0, &[],));
        assert!(!realtime_batch_fits(0, parameter_limit + 1, 0, &[],));
        assert!(!realtime_batch_fits(
            0,
            PARAMETER_POINTS_PER_ID + 1,
            PARAMETER_POINTS_PER_ID + 1,
            &[],
        ));
        assert!(!realtime_batch_fits(
            0,
            PARAMETER_POINTS_PER_ID + 1,
            0,
            &[PARAMETER_POINTS_PER_ID as u8 + 1],
        ));
    }

    #[test]
    fn event_validation_and_note_tracking_match_the_midi_conversion() {
        assert!(note_event_is_valid(
            &NoteEvent::NoteOn {
                frame: 0,
                pitch: 127,
                velocity: 1.0,
            },
            64,
        ));
        assert!(!note_event_is_valid(
            &NoteEvent::NoteOff {
                frame: 0,
                pitch: 128,
            },
            64,
        ));
        assert!(!note_event_is_valid(
            &NoteEvent::Controller {
                frame: 0,
                number: 128,
                value: 0.5,
            },
            64,
        ));
        assert!(!note_event_is_valid(
            &NoteEvent::AllNotesOff { frame: 1 },
            64,
        ));
        assert!(!note_event_is_valid(
            &NoteEvent::NoteOff {
                frame: 64,
                pitch: 60,
            },
            64,
        ));
        assert!(!note_event_is_valid(
            &NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: f32::NAN,
            },
            64,
        ));
        assert!(!note_event_is_valid(
            &NoteEvent::Controller {
                frame: 0,
                number: 1,
                value: f32::INFINITY,
            },
            64,
        ));
        assert!(!note_event_is_valid(
            &NoteEvent::PitchBend {
                frame: 0,
                semitones: f32::NAN,
            },
            64,
        ));
        assert_eq!(midi_velocity(0.0), 0);
        assert_eq!(midi_velocity(f32::NAN), 0);
        assert_eq!(midi_velocity(1.0), 127);
    }

    #[test]
    fn process_context_is_preflighted_before_realtime_queues_are_touched() {
        let prepare = PrepareContext::new(48_000.0, 512, 2);
        let valid = ProcessContext::realtime(48_000.0, 256, 0, 120.0, true);
        assert!(process_context_is_valid(&valid, 256, &prepare));

        let mut invalid = valid;
        invalid.bpm = f64::NAN;
        assert!(!process_context_is_valid(&invalid, 256, &prepare));
        invalid = valid;
        invalid.sample_rate = 44_100.0;
        assert!(!process_context_is_valid(&invalid, 256, &prepare));
        invalid = valid;
        invalid.block_frames = 128;
        assert!(!process_context_is_valid(&invalid, 256, &prepare));
        let oversized = ProcessContext::realtime(48_000.0, 513, 0, 120.0, true);
        assert!(!process_context_is_valid(&oversized, 513, &prepare));
    }

    #[test]
    fn parameter_snapshot_publishes_the_latest_value_without_a_mutex() {
        let snapshot = ParameterSnapshot::new(&[0.25]);
        let before = snapshot.revision();

        assert!(snapshot.publish(0, 0.75));
        assert_eq!(snapshot.value(0), Some(0.75));
        assert_ne!(snapshot.revision(), before);
        let unchanged = snapshot.revision();
        assert!(!snapshot.publish(0, 0.75));
        assert_eq!(snapshot.revision(), unchanged);
    }

    #[test]
    fn parameter_snapshot_release_publishes_to_a_concurrent_renderer() {
        const WRITES: u64 = 10_000;
        let snapshot = Arc::new(ParameterSnapshot::new(&[0.0]));
        let target_revision = snapshot.revision() + WRITES;
        let writer_snapshot = Arc::clone(&snapshot);
        let writer = std::thread::spawn(move || {
            for index in 1..=WRITES {
                assert!(writer_snapshot.publish(0, index as f32));
            }
        });

        while snapshot.revision() < target_revision {
            std::hint::spin_loop();
        }
        assert_eq!(snapshot.value(0), Some(WRITES as f32));
        writer.join().unwrap();
    }

    #[test]
    fn renderer_failure_preserves_the_first_reason_until_the_ui_takes_it() {
        let failure = AtomicU32::new(Vst3RenderFailure::NONE);
        assert!(
            failure
                .compare_exchange(
                    Vst3RenderFailure::NONE,
                    Vst3RenderFailure::CAPACITY,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        );
        let _ = failure.compare_exchange(
            Vst3RenderFailure::NONE,
            Vst3RenderFailure::PROCESSING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        assert_eq!(
            Vst3RenderFailure::decode(failure.swap(Vst3RenderFailure::NONE, Ordering::AcqRel)),
            Some(Vst3RenderFailure::RealtimeCapacityExceeded)
        );
        assert_eq!(
            Vst3RenderFailure::decode(failure.load(Ordering::Acquire)),
            None
        );
    }

    #[test]
    fn render_owned_values_are_destroyed_only_after_owner_thread_service() {
        struct DropProbe(Arc<AtomicUsize>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let drops = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let returned = Arc::new(ArrayQueue::new(1));
        let deferred = DeferredOwnerDrop::new(
            DropProbe(Arc::clone(&drops)),
            Arc::clone(&returned),
            &active,
        );

        std::thread::spawn(move || drop(deferred)).join().unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert_eq!(active.load(Ordering::Relaxed), 0);

        let returned = returned.pop().expect("render value returns to its owner");
        drop(ManuallyDrop::into_inner(returned));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
