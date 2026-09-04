//! Hosting third-party VST3 plugins for Auris Studio.
//!
//! The VST3 SDK has used the MIT license since version 3.8. This crate adapts the safe,
//! MIT-licensed `vst3-host` API to Auris' format-independent instrument and effect traits.

#![warn(missing_docs)]

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use auris_core::buffer::AudioBuffer;
use auris_core::param::{ParamDescriptor, ParamId, ParamUnit};
use auris_core::plugin::{
    Effect, Instrument, NoteEvent, Parameterized, PluginCategory, PluginDescriptor, PluginKind,
    PrepareContext, ProcessContext,
};
use thiserror::Error;
use vst3_host::{
    BusAudioBuffers, BusDirection, MediaType, MidiChannel, MidiEvent, Parameter, Plugin, Vst3Host,
};

/// Prefix used for VST3 class ids stored in Auris project files.
pub const ID_PREFIX: &str = "vst3:";

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

/// A loaded VST3 instance retained by the editing session.
///
/// Rendering wrappers share the instance. The mutex is normally uncontended: editing and state
/// operations happen on the UI thread between audio callbacks. The render path uses `try_lock`,
/// so a slow editor operation can cause one bypassed/silent block but can never block audio.
pub struct Vst3Plugin {
    shared: Arc<Shared>,
    window: Option<vst3_host::PluginWindow>,
}

struct Shared {
    plugin: Arc<Mutex<Plugin>>,
    info: Vst3PluginInfo,
    descriptor: PluginDescriptor,
    parameters: Arc<Vec<ParamDescriptor>>,
    parameter_ids: Arc<Vec<u32>>,
    prepare: PrepareContext,
    sidechain: bool,
}

impl std::fmt::Debug for Vst3Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vst3Plugin")
            .field("info", &self.shared.info)
            .finish_non_exhaustive()
    }
}

impl Vst3Plugin {
    /// Loads and configures one VST3 audio class.
    pub fn load(path: &Path, class_id: &str, prepare: &PrepareContext) -> Result<Self, Vst3Error> {
        let mut host = Vst3Host::builder()
            .sample_rate(prepare.sample_rate)
            .block_size(prepare.max_block_frames.max(1))
            .build()?;
        let mut plugin = host.load_plugin_class(path, class_id)?;
        let raw_parameters = plugin.get_parameters()?;
        let (parameters, parameter_ids) = describe_parameters(&raw_parameters);
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

        let layout = plugin.audio_bus_layout()?;
        let sidechain = layout.inputs.len() > 1;
        for index in 1..layout.inputs.len() {
            plugin.set_bus_active(MediaType::Audio, BusDirection::Input, index as i32, true)?;
        }
        Ok(Self {
            shared: Arc::new(Shared {
                descriptor: info.descriptor(),
                info,
                plugin: Arc::new(Mutex::new(plugin)),
                parameters: Arc::new(parameters),
                parameter_ids: Arc::new(parameter_ids),
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
        self.lock()?
            .set_parameter(vst_id, descriptor.clamp(value) as f64)?;
        Ok(())
    }
    /// Serializes the plugin's opaque project state.
    pub fn save_state(&self) -> Result<Vec<u8>, Vst3Error> {
        Ok(self.lock()?.save_state()?)
    }
    /// Restores an opaque state blob previously produced by this class.
    pub fn load_state(&self, bytes: &[u8]) -> Result<(), Vst3Error> {
        self.lock()?.load_state(bytes)?;
        Ok(())
    }
    /// Creates an effect wrapper for the render graph.
    pub fn effect(&self) -> Result<Vst3Effect, Vst3Error> {
        self.lock()?.start_processing()?;
        Vst3Effect::new(self.shared.clone())
    }
    /// Creates an instrument wrapper for the render graph.
    pub fn instrument(&self) -> Result<Vst3Instrument, Vst3Error> {
        self.lock()?.start_processing()?;
        Vst3Instrument::new(self.shared.clone())
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
        Arc::strong_count(&self.shared) == 1
    }
    fn lock(&self) -> Result<MutexGuard<'_, Plugin>, Vst3Error> {
        self.shared.plugin.lock().map_err(|_| Vst3Error::Busy)
    }
}

/// A VST3 audio effect as seen by the render graph.
pub struct Vst3Effect(Bridge);
impl std::fmt::Debug for Vst3Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Vst3Effect")
            .field(&self.0.shared.info.name)
            .finish()
    }
}
impl Vst3Effect {
    fn new(shared: Arc<Shared>) -> Result<Self, Vst3Error> {
        Ok(Self(Bridge::new(shared)?))
    }
}

/// A VST3 software instrument as seen by the render graph.
pub struct Vst3Instrument(Bridge);
impl std::fmt::Debug for Vst3Instrument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Vst3Instrument")
            .field(&self.0.shared.info.name)
            .finish()
    }
}
impl Vst3Instrument {
    fn new(shared: Arc<Shared>) -> Result<Self, Vst3Error> {
        Ok(Self(Bridge::new(shared)?))
    }
}

struct Bridge {
    shared: Arc<Shared>,
    buffers: BusAudioBuffers,
    values: Vec<f32>,
    latency: usize,
}

impl Bridge {
    fn new(shared: Arc<Shared>) -> Result<Self, Vst3Error> {
        let plugin = shared.plugin.lock().map_err(|_| Vst3Error::Busy)?;
        let buffers = plugin.create_bus_audio_buffers(shared.prepare.max_block_frames.max(1))?;
        let latency = plugin.latency_samples() as usize;
        let values = shared
            .parameter_ids
            .iter()
            .zip(shared.parameters.iter())
            .map(|(id, descriptor)| {
                plugin
                    .get_parameter(*id)
                    .ok()
                    .map_or(descriptor.default, |value| value as f32)
            })
            .collect();
        drop(plugin);
        Ok(Self {
            shared,
            buffers,
            values,
            latency,
        })
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        let Some(descriptor) = self.shared.parameters.get(id.index()) else {
            return;
        };
        let value = descriptor.clamp(value);
        self.values[id.index()] = value;
        let Some(&vst_id) = self.shared.parameter_ids.get(id.index()) else {
            return;
        };
        if let Ok(mut plugin) = self.shared.plugin.try_lock() {
            let _ = plugin.set_parameter(vst_id, value as f64);
        }
    }

    fn render(
        &mut self,
        buffer: &mut AudioBuffer,
        events: &[NoteEvent],
        overwrite: bool,
        sidechain: Option<&AudioBuffer>,
        ctx: &ProcessContext,
    ) {
        let frames = buffer
            .frame_count()
            .min(self.shared.prepare.max_block_frames);
        if frames == 0 {
            return;
        }
        let Ok(mut plugin) = self.shared.plugin.try_lock() else {
            if overwrite {
                buffer.clear();
            }
            return;
        };
        let _ = plugin.set_tempo(ctx.bpm);
        let _ = plugin.set_playing(ctx.is_playing);
        for event in events {
            let midi = translate_event(*event);
            let _ = plugin.send_midi_event_at(midi, event.frame() as i32);
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
        if plugin.process_bus_audio(&mut self.buffers).is_err() {
            if overwrite {
                buffer.clear();
            }
            return;
        }
        let Some(main) = self.buffers.outputs.iter().find(|bus| bus.active) else {
            if overwrite {
                buffer.clear();
            }
            return;
        };
        deliver(main.channels.as_slice(), buffer, frames, overwrite);
    }
}

impl Parameterized for Vst3Effect {
    fn parameters(&self) -> &[ParamDescriptor] {
        &self.0.shared.parameters
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
        self.0.shared.descriptor.clone()
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        self.0.render(buffer, &[], false, None, ctx);
    }
    fn wants_sidechain(&self) -> bool {
        self.0.shared.sidechain
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
        &self.0.shared.parameters
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
        self.0.shared.descriptor.clone()
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
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
            velocity: (velocity.clamp(0.0, 1.0) * 127.0).round() as u8,
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
}
