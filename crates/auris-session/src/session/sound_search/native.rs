//! The half of the sound catalog that runs third-party code.
//!
//! Listing a plugin's presets, measuring one of its sounds and rendering an audition note all
//! instantiate a CLAP or VST3 binary, and a binary can crash or hang the process that loads it.
//! Every frontend therefore does this work in the plugin worker process (see
//! [`crate::plugin_worker`]) and gets back plain data: the window, the command line and the MCP
//! server all see the same catalog and the same measurements, and a misbehaving plugin takes
//! down only its own child.
//!
//! The playing code is shared with the in-process path: a built-in instrument, a SoundFont
//! preset or a patch made only of built-in parts is measured by the same functions in the
//! caller's process, because nothing in it is somebody else's code.

use super::super::SoundEnvelope;
use super::super::envelope;
use super::super::timbre::{TimbreMapControl, render_reference};
use crate::{SoundPreset, TimbreCharacter};
use auris_core::PluginRegistry;
use auris_core::plugin::{
    Effect, Instrument, NoteEvent, PluginKind, PluginState, PrepareContext, ProcessContext,
};
use auris_core::{AudioBuffer, PresetRef, SoundFontId};
use auris_dsp::timbre::{timbre_character, timbre_features};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Reference notes' hold, in seconds; the same as the map's.
const HOLD: f64 = 0.6;
/// Block size every render uses.
const BLOCK: usize = 512;
/// Most sounds one plugin file may contribute to a listing.
const LISTING_SOUND_LIMIT: usize = 16_384;
/// Most diagnostics one listing may carry.
const LISTING_ERROR_LIMIT: usize = 256;

/// [`auris_clap::ClapPreset`] as it crosses the process boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClapPresetWire {
    name: String,
    plugin_ids: Vec<String>,
    location: Option<String>,
    load_key: Option<String>,
    features: Vec<String>,
}

impl From<&auris_clap::ClapPreset> for ClapPresetWire {
    fn from(preset: &auris_clap::ClapPreset) -> Self {
        Self {
            name: preset.name.clone(),
            plugin_ids: preset.plugin_ids.clone(),
            location: preset.location.clone(),
            load_key: preset.load_key.clone(),
            features: preset.features.clone(),
        }
    }
}

impl From<ClapPresetWire> for auris_clap::ClapPreset {
    fn from(preset: ClapPresetWire) -> Self {
        Self {
            name: preset.name,
            plugin_ids: preset.plugin_ids,
            location: preset.location,
            load_key: preset.load_key,
            features: preset.features,
        }
    }
}

/// [`auris_vst3::Vst3Preset`] as it crosses the process boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Vst3PresetWire {
    Program { unit: i32, index: i32 },
    File(PathBuf),
}

impl From<&auris_vst3::Vst3Preset> for Vst3PresetWire {
    fn from(preset: &auris_vst3::Vst3Preset) -> Self {
        match preset {
            auris_vst3::Vst3Preset::Program { unit, index } => Self::Program {
                unit: *unit,
                index: *index,
            },
            auris_vst3::Vst3Preset::File(path) => Self::File(path.clone()),
        }
    }
}

impl From<Vst3PresetWire> for auris_vst3::Vst3Preset {
    fn from(preset: Vst3PresetWire) -> Self {
        match preset {
            Vst3PresetWire::Program { unit, index } => Self::Program { unit, index },
            Vst3PresetWire::File(path) => Self::File(path),
        }
    }
}

/// A sound the worker process is asked to play.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum WorkerSound {
    /// A CLAP instrument, at its default state or with one advertised preset loaded.
    Clap {
        file: PathBuf,
        id: String,
        preset: Option<ClapPresetWire>,
    },
    /// A VST3 instrument, at its default state or with one program or preset file loaded.
    Vst3 {
        file: PathBuf,
        id: String,
        preset: Option<Vst3PresetWire>,
    },
    /// A saved instrument and insert chain, some of which lives in a file.
    Patch(SoundPreset),
}

/// One sound a plugin file offers, as its listing names it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ListedSound {
    pub(crate) name: String,
    pub(crate) library: String,
    pub(crate) tags: String,
    pub(crate) sound: WorkerSound,
}

/// Every instrument and preset one plugin file offers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct PluginListing {
    pub(crate) sounds: Vec<ListedSound>,
    /// VST3 instrument classes, for matching standard preset files the parent found on disk.
    pub(crate) vst3_classes: Vec<(String, String)>,
    pub(crate) errors: Vec<String>,
    pub(crate) truncated: bool,
}

impl PluginListing {
    fn push(&mut self, sound: ListedSound) -> bool {
        if self.sounds.len() >= LISTING_SOUND_LIMIT {
            self.truncated = true;
            return false;
        }
        self.sounds.push(sound);
        true
    }

    fn error(&mut self, error: impl Into<String>) {
        if self.errors.len() < LISTING_ERROR_LIMIT {
            self.errors.push(error.into());
        }
    }
}

/// What indexing one sound yields, on either side of the process boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Measurement {
    /// The six reference notes' timbre features, concatenated.
    pub(crate) features: Vec<f64>,
    /// Their mean fixed-scale descriptors.
    pub(crate) character: TimbreCharacter,
    /// The measured envelope; `None` when no envelope pitch sounded.
    pub(crate) envelope: Option<SoundEnvelope>,
}

/// Lists one plugin file's instruments and their presets. Runs in the worker process only.
pub(crate) fn list_plugin(
    format: crate::PluginFormat,
    path: &Path,
    sample_rate: f64,
) -> Result<PluginListing, String> {
    match format {
        crate::PluginFormat::Clap => list_clap(path),
        crate::PluginFormat::Vst3 => list_vst3(path, sample_rate),
    }
}

fn list_clap(path: &Path) -> Result<PluginListing, String> {
    // SAFETY: this is the isolated worker process; a malformed plugin ends it, not the caller.
    let library = unsafe { auris_clap::ClapLibrary::load(path) }.map_err(|e| e.to_string())?;
    let mut listing = PluginListing::default();
    let mut plugins = library.plugins().map_err(|e| e.to_string())?;
    plugins.retain(|info| info.kind == PluginKind::Instrument);
    if plugins.is_empty() {
        return Ok(listing);
    }
    let metadata = library.presets();
    for error in metadata.errors {
        listing.error(error);
    }
    for info in plugins {
        let library_name = format!("{} / {}", info.vendor, info.name);
        if !listing.push(ListedSound {
            name: format!("{} (default)", info.name),
            library: library_name.clone(),
            tags: info.description.clone(),
            sound: WorkerSound::Clap {
                file: path.into(),
                id: info.clap_id.clone(),
                preset: None,
            },
        }) {
            break;
        }
        match library.instantiate(&info.clap_id) {
            Ok(plugin) if plugin.supports_preset_loading() => {}
            Ok(_) => {
                listing.error(format!("{}: no CLAP preset-load extension", info.name));
                continue;
            }
            Err(error) => {
                listing.error(format!("{}: {error}", info.name));
                continue;
            }
        }
        for preset in metadata
            .presets
            .iter()
            .filter(|preset| preset.plugin_ids.contains(&info.clap_id))
        {
            if !listing.push(ListedSound {
                name: preset.name.clone(),
                library: library_name.clone(),
                tags: preset.features.join(" "),
                sound: WorkerSound::Clap {
                    file: path.into(),
                    id: info.clap_id.clone(),
                    preset: Some(preset.into()),
                },
            }) {
                return Ok(listing);
            }
        }
    }
    Ok(listing)
}

fn list_vst3(path: &Path, sample_rate: f64) -> Result<PluginListing, String> {
    let mut listing = PluginListing::default();
    let mut plugins = auris_vst3::plugins_in(path).map_err(|e| e.to_string())?;
    plugins.retain(|info| info.kind == PluginKind::Instrument);
    for info in plugins {
        let library_name = format!("{} / {}", info.vendor, info.name);
        listing
            .vst3_classes
            .push((info.class_id.clone(), library_name.clone()));
        if !listing.push(ListedSound {
            name: format!("{} (default)", info.name),
            library: library_name.clone(),
            tags: String::new(),
            sound: WorkerSound::Vst3 {
                file: path.into(),
                id: info.class_id.clone(),
                preset: None,
            },
        }) {
            break;
        }
        let programs = auris_vst3::Vst3Plugin::load(
            path,
            &info.class_id,
            &PrepareContext::new(sample_rate, BLOCK, 2),
        )
        .and_then(|plugin| plugin.presets());
        match programs {
            Ok(programs) => {
                for (name, preset) in programs {
                    if !listing.push(ListedSound {
                        name,
                        library: library_name.clone(),
                        tags: String::new(),
                        sound: WorkerSound::Vst3 {
                            file: path.into(),
                            id: info.class_id.clone(),
                            preset: Some((&preset).into()),
                        },
                    }) {
                        return Ok(listing);
                    }
                }
            }
            Err(error) => listing.error(format!("{}: {error}", info.name)),
        }
    }
    Ok(listing)
}

/// Measures a sound that needs third-party code or a file. Runs in the worker process only.
pub(crate) fn measure_worker_sound(
    sound: &WorkerSound,
    sample_rate: f64,
) -> Result<Measurement, String> {
    measure_worker_sound_with(sound, sample_rate, Reuse::Reactivate)
}

/// [`measure_worker_sound`], with `reuse` choosing how each of the nine notes gets its
/// instrument.
///
/// Building a plugin instance is nearly all of what measuring one costs — Surge XT takes about
/// 0.3 s to build and 4 ms to render a reference note — so a lone CLAP instrument is built once
/// and deactivated and activated again before every note, which starts its processing afresh.
/// That is not the same as a new instance: many presets advance random or free-running state
/// from note to note. Neither is a fresh instance a reference, though: it is the plugin's first
/// note, and a fresh instance that has played one note first differs from it as often as a
/// reactivated one does (13 and 16 of 27 Surge presets past 0.1 on an axis). Anything else — a
/// VST3 instrument, whose render half is a new instance anyway, or a saved sound with inserts —
/// gets a fresh instance per note.
fn measure_worker_sound_with(
    sound: &WorkerSound,
    sample_rate: f64,
    reuse: Reuse,
) -> Result<Measurement, String> {
    let control = TimbreMapControl::default();
    let prepare = PrepareContext::new(sample_rate, BLOCK, 2);
    let mut kept: Option<Played> = None;
    measure(sample_rate, &control, &mut |play| {
        let reactivated = match (reuse, &mut kept) {
            (Reuse::Reactivate, Some(played)) => played.reactivate(&prepare)?,
            _ => false,
        };
        if !reactivated {
            #[cfg_attr(not(test), allow(unused_mut))]
            let mut played = build_worker_sound(sound, &prepare)?;
            #[cfg(test)]
            if reuse == Reuse::Warm {
                render_reference(played.chain(), sample_rate, 60, 0.85, &control)
                    .map_err(|e| e.to_string())?;
                settle(played.chain(), sample_rate)?;
            }
            kept = Some(played);
        }
        play(kept.as_mut().expect("built above").chain())
    })
}

/// How the worker gets an instrument for each measured note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reuse {
    /// A fresh instance for every note.
    #[cfg_attr(not(test), allow(dead_code))]
    Fresh,
    /// One instance, deactivated and activated again, where the plugin allows it.
    Reactivate,
    /// A fresh instance that has already played one note and fallen silent.
    #[cfg(test)]
    Warm,
}

/// Output below this peak counts as silence while settling (-100 dBFS).
#[cfg(test)]
const SETTLED_PEAK: f32 = 1e-5;
/// How long the output must stay silent to count as settled.
#[cfg(test)]
const SETTLED_SECONDS: f64 = 0.1;
/// The longest settling may take; a plugin that never falls silent is measured as it is.
#[cfg(test)]
const SETTLE_LIMIT_SECONDS: f64 = 4.0;

/// Returns a played instrument to rest between measured notes: reset, then render silence until
/// [`SETTLED_SECONDS`] in a row stay below [`SETTLED_PEAK`].
#[cfg(test)]
fn settle(instrument: &mut dyn Instrument, sample_rate: f64) -> Result<(), String> {
    instrument.reset();
    let mut block = AudioBuffer::stereo(BLOCK, sample_rate);
    let needed = (SETTLED_SECONDS * sample_rate / BLOCK as f64).ceil() as usize;
    let limit = (SETTLE_LIMIT_SECONDS * sample_rate / BLOCK as f64).ceil() as usize;
    let mut quiet = 0;
    for index in 0..limit {
        block.set_frame_count(BLOCK);
        block.clear();
        instrument.process(
            &[],
            &mut block,
            &ProcessContext {
                sample_rate,
                block_frames: BLOCK,
                playhead_samples: (index * BLOCK) as u64,
                bpm: 120.0,
                is_playing: true,
                is_offline: true,
            },
        );
        block.sanitize();
        quiet = if block.peak() < SETTLED_PEAK {
            quiet + 1
        } else {
            0
        };
        if quiet >= needed {
            break;
        }
    }
    instrument.reset();
    Ok(())
}

/// Renders one level-unmatched audition note. Runs in the worker process only.
pub(crate) fn preview_worker_sound(
    sound: &WorkerSound,
    sample_rate: f64,
    pitch: u8,
) -> Result<Vec<Vec<f32>>, String> {
    let control = TimbreMapControl::default();
    let prepare = PrepareContext::new(sample_rate, BLOCK, 2);
    let mut played = build_worker_sound(sound, &prepare)?;
    let audio = render_reference(played.chain(), sample_rate, pitch, 0.85, &control)
        .map_err(|e| e.to_string())?;
    Ok(audio.channels().to_vec())
}

/// Hands a freshly built instrument to its argument; called once per rendered note.
pub(crate) type Play<'a> =
    dyn FnMut(&mut dyn FnMut(&mut dyn Instrument) -> Result<(), String>) -> Result<(), String> + 'a;

/// Six reference notes for the timbre features, then the three envelope notes, each through a
/// fresh instance so no note hears the one before it.
pub(crate) fn measure(
    sample_rate: f64,
    control: &TimbreMapControl,
    play: &mut Play<'_>,
) -> Result<Measurement, String> {
    let mut features = Vec::new();
    let mut characters = Vec::new();
    for pitch in [48, 60, 72] {
        for velocity in [0.45, 0.85] {
            control.check().map_err(|e| e.to_string())?;
            let mut audio = None;
            play(&mut |instrument| {
                audio = Some(
                    render_reference(instrument, sample_rate, pitch, velocity, control)
                        .map_err(|e| e.to_string())?,
                );
                Ok(())
            })?;
            let audio = audio.ok_or("the instrument was not played")?;
            let values = timbre_features(&audio, HOLD)
                .map_err(str::to_string)?
                .ok_or("Silent reference trigger")?;
            characters.push(timbre_character(&audio, HOLD, &values));
            features.extend(values);
        }
    }
    let envelope = envelope::measure_sound(|pitch| {
        let mut measured = None;
        play(&mut |instrument| {
            measured = envelope::measure_note(instrument, sample_rate, pitch, control)
                .map_err(|e| e.to_string())?;
            Ok(())
        })?;
        Ok(measured)
    })?;
    Ok(Measurement {
        features,
        character: TimbreCharacter::mean(&characters),
        envelope,
    })
}

/// An instrument followed by its enabled inserts, played as one instrument.
pub(crate) struct Chain {
    instrument: Box<dyn Instrument>,
    effects: Vec<Box<dyn Effect>>,
}

/// A chain together with the third-party objects its halves render for. Those are released
/// after the halves, in the order their hosts require, and never leave the worker process.
struct Played {
    chain: Option<Chain>,
    hosts: Vec<Host>,
}

enum Host {
    Clap(auris_clap::ClapPlugin),
    /// Never read: it is held so the plugin outlives the halves rendering for it.
    Vst3(#[allow(dead_code)] auris_vst3::Vst3Plugin),
}

impl Drop for Played {
    fn drop(&mut self) {
        self.chain = None;
        for host in &mut self.hosts {
            if let Host::Clap(plugin) = host {
                let _ = plugin.release();
            }
        }
    }
}

impl Played {
    /// Deactivates and activates a lone CLAP instrument again, which starts its processing
    /// afresh without building a new instance. `false` for anything else.
    fn reactivate(&mut self, prepare: &PrepareContext) -> Result<bool, String> {
        let [Host::Clap(plugin)] = self.hosts.as_mut_slice() else {
            return Ok(false);
        };
        if self
            .chain
            .as_ref()
            .is_none_or(|chain| !chain.effects.is_empty())
        {
            return Ok(false);
        }
        self.chain = None;
        if !plugin.release() {
            return Err("the plugin stayed active after its instrument was dropped".into());
        }
        let instrument = plugin
            .activate_instrument(prepare)
            .map_err(|e| e.to_string())?;
        let mut chain = Chain {
            instrument: Box::new(instrument),
            effects: Vec::new(),
        };
        chain.prepare(prepare);
        self.chain = Some(chain);
        Ok(true)
    }

    fn chain(&mut self) -> &mut Chain {
        self.chain
            .as_mut()
            .expect("a played sound keeps its chain until it is dropped")
    }
}

impl auris_core::plugin::Parameterized for Chain {
    fn parameters(&self) -> &[auris_core::ParamDescriptor] {
        self.instrument.parameters()
    }
    fn param(&self, id: auris_core::ParamId) -> f32 {
        self.instrument.param(id)
    }
    fn set_param(&mut self, id: auris_core::ParamId, value: f32) {
        self.instrument.set_param(id, value);
    }
}

impl Instrument for Chain {
    fn descriptor(&self) -> auris_core::PluginDescriptor {
        self.instrument.descriptor()
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.instrument.prepare(ctx);
        for effect in &mut self.effects {
            effect.prepare(ctx);
        }
    }

    fn reset(&mut self) {
        self.instrument.reset();
        for effect in &mut self.effects {
            effect.reset();
        }
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        self.instrument.process(events, out, ctx);
        for effect in &mut self.effects {
            effect.process(out, ctx);
        }
    }
}

fn build_worker_sound(sound: &WorkerSound, prepare: &PrepareContext) -> Result<Played, String> {
    let mut hosts = Vec::new();
    let mut chain = match sound {
        WorkerSound::Clap { file, id, preset } => {
            let preset = preset.clone().map(auris_clap::ClapPreset::from);
            // SAFETY: this is the isolated worker process.
            let library =
                unsafe { auris_clap::ClapLibrary::load(file) }.map_err(|e| e.to_string())?;
            let mut plugin = library.instantiate(id).map_err(|e| e.to_string())?;
            if let Some(preset) = &preset {
                plugin.load_preset(preset)?;
            }
            let instrument = plugin
                .activate_instrument(prepare)
                .map_err(|e| e.to_string())?;
            hosts.push(Host::Clap(plugin));
            Chain {
                instrument: Box::new(instrument),
                effects: Vec::new(),
            }
        }
        WorkerSound::Vst3 { file, id, preset } => {
            let plugin =
                auris_vst3::Vst3Plugin::load(file, id, prepare).map_err(|e| e.to_string())?;
            if let Some(preset) = preset {
                plugin
                    .load_preset(&preset.clone().into())
                    .map_err(|e| e.to_string())?;
            }
            let instrument = plugin.offline_instrument().map_err(|e| e.to_string())?;
            hosts.push(Host::Vst3(plugin));
            Chain {
                instrument: Box::new(instrument),
                effects: Vec::new(),
            }
        }
        WorkerSound::Patch(patch) => {
            let fonts = auris_sampler::SoundFontBank::shared();
            let mut state = patch.state.clone();
            if let Some(font) = &patch.soundfont {
                let loaded = crate::read_soundfont(&font.path).map_err(|e| e.to_string())?;
                fonts.insert(SoundFontId(0), loaded.0);
                auris_sampler::store_preset(
                    &mut state,
                    PresetRef {
                        font: SoundFontId(0),
                        bank: font.bank,
                        patch: font.patch,
                    },
                );
            }
            let registry = crate::default_registry(fonts);
            patch_chain(patch, &state, &registry, &mut |id, file, state, kind| {
                hosted_part(&mut hosts, id, file, state, kind, prepare)
            })?
        }
    };
    chain.prepare(prepare);
    Ok(Played {
        chain: Some(chain),
        hosts,
    })
}

/// Builds a patch made only of built-in parts, in the caller's process.
pub(crate) fn build_builtin_patch(
    patch: &SoundPreset,
    registry: &PluginRegistry,
    prepare: &PrepareContext,
) -> Result<Chain, String> {
    let mut chain = patch_chain(patch, &patch.state, registry, &mut |id, _, _, _| {
        Err(format!("{id} is not a built-in plugin"))
    })?;
    chain.prepare(prepare);
    Ok(chain)
}

/// Whether a patch can be played without loading a file or third-party code.
pub(crate) fn patch_is_builtin(patch: &SoundPreset) -> bool {
    patch.file.is_none()
        && patch.soundfont.is_none()
        && patch.effects.iter().all(|effect| effect.file.is_none())
}

enum Part {
    Instrument(Box<dyn Instrument>),
    Effect(Box<dyn Effect>),
}

type HostedPart<'a> = dyn FnMut(&str, &Path, &PluginState, PluginKind) -> Result<Part, String> + 'a;

fn patch_chain(
    patch: &SoundPreset,
    state: &PluginState,
    registry: &PluginRegistry,
    hosted: &mut HostedPart<'_>,
) -> Result<Chain, String> {
    let instrument = match &patch.file {
        Some(file) => match hosted(&patch.instrument_id, file, state, PluginKind::Instrument)? {
            Part::Instrument(instrument) => instrument,
            Part::Effect(_) => return Err("the patch's instrument is an effect".into()),
        },
        None => {
            let mut instrument = registry
                .create_instrument(&patch.instrument_id)
                .map_err(|e| e.to_string())?;
            instrument.load_state(state);
            instrument
        }
    };
    let mut effects = Vec::new();
    for effect in patch.effects.iter().filter(|effect| effect.enabled) {
        effects.push(match &effect.file {
            Some(file) => {
                match hosted(&effect.effect_id, file, &effect.state, PluginKind::Effect)? {
                    Part::Effect(effect) => effect,
                    Part::Instrument(_) => return Err("an insert is an instrument".into()),
                }
            }
            None => {
                let mut built = registry
                    .create_effect(&effect.effect_id)
                    .map_err(|e| e.to_string())?;
                built.load_state(&effect.state);
                built
            }
        });
    }
    Ok(Chain {
        instrument,
        effects,
    })
}

fn hosted_part(
    hosts: &mut Vec<Host>,
    id: &str,
    file: &Path,
    state: &PluginState,
    kind: PluginKind,
    prepare: &PrepareContext,
) -> Result<Part, String> {
    let bytes = state.hosted_bytes();
    if let Some(clap_id) = id.strip_prefix(auris_clap::ID_PREFIX) {
        // SAFETY: this is the isolated worker process.
        let library = unsafe { auris_clap::ClapLibrary::load(file) }.map_err(|e| e.to_string())?;
        let mut plugin = library.instantiate(clap_id).map_err(|e| e.to_string())?;
        if let Some(bytes) = &bytes {
            plugin.load_state(bytes).map_err(|e| e.to_string())?;
        }
        let part = match kind {
            PluginKind::Instrument => Part::Instrument(Box::new(
                plugin
                    .activate_instrument(prepare)
                    .map_err(|e| e.to_string())?,
            )),
            _ => Part::Effect(Box::new(
                plugin.activate(prepare).map_err(|e| e.to_string())?,
            )),
        };
        hosts.push(Host::Clap(plugin));
        Ok(part)
    } else if let Some(class) = id.strip_prefix(auris_vst3::ID_PREFIX) {
        let plugin =
            auris_vst3::Vst3Plugin::load(file, class, prepare).map_err(|e| e.to_string())?;
        if let Some(bytes) = &bytes {
            plugin.load_state(bytes).map_err(|e| e.to_string())?;
        }
        let part = match kind {
            PluginKind::Instrument => Part::Instrument(Box::new(
                plugin.offline_instrument().map_err(|e| e.to_string())?,
            )),
            _ => Part::Effect(Box::new(
                plugin.offline_effect().map_err(|e| e.to_string())?,
            )),
        };
        hosts.push(Host::Vst3(plugin));
        Ok(part)
    } else {
        Err(format!("{id} names no hosted plugin format"))
    }
}

/// Rebuilds an audition buffer from the channels a worker sent back.
pub(crate) fn audio_from_channels(
    channels: Vec<Vec<f32>>,
    sample_rate: f64,
) -> Result<AudioBuffer, String> {
    let frames = channels.first().map_or(0, Vec::len);
    if channels.len() != 2 || channels.iter().any(|channel| channel.len() != frames) {
        return Err("the plugin worker returned malformed audio".into());
    }
    let mut audio = AudioBuffer::stereo(frames, sample_rate);
    for (index, channel) in channels.into_iter().enumerate() {
        audio.channel_mut(index).copy_from_slice(&channel);
    }
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const SURGE: &str = r"C:\Program Files\Common Files\CLAP\Surge Synth Team\Surge XT.clap";

    /// Splits a plugin measurement's cost, and compares one reused instance with nine fresh
    /// ones over real presets. Run on demand: it loads an installed third-party plugin into the
    /// test process.
    #[test]
    #[ignore = "Requires installed Surge XT and executes third-party code"]
    fn reusing_one_instance_matches_fresh_instances_on_installed_presets() {
        let rate = 48_000.;
        let prepare = PrepareContext::new(rate, BLOCK, 2);
        let listing = list_plugin(crate::PluginFormat::Clap, Path::new(SURGE), rate).unwrap();
        let control = TimbreMapControl::default();
        let first = &listing.sounds[0].sound;
        let started = Instant::now();
        let mut played = build_worker_sound(first, &prepare).unwrap();
        let built = started.elapsed();
        let started = Instant::now();
        render_reference(played.chain(), rate, 60, 0.85, &control).unwrap();
        let rendered = started.elapsed();
        drop(played);
        eprintln!("build {built:?}, render one reference note {rendered:?}");

        let sample = 27;
        let step = (listing.sounds.len() / sample).max(1);
        let axes = |m: &Measurement| {
            crate::SoundAxes::new(m.character, m.envelope.as_ref().map(|e| &e.summary)).position()
        };
        let gap = |a: [f64; 3], b: [f64; 3]| {
            [
                (a[0] - b[0]).abs(),
                (a[1] - b[1]).abs(),
                (a[2] - b[2]).abs(),
            ]
        };
        // A difference counts only beyond one 5 ms attack step (0.35 on the attack axis near
        // 1 ms) and 0.1 on the other axes.
        let beyond = |g: [f64; 3]| g[0] > 0.1 || g[1] > 0.36 || g[2] > 0.1;
        let (mut measured, mut warm_beyond, mut reactivated_beyond, mut both) = (0, 0, 0, 0);
        let mut times = [std::time::Duration::ZERO; 2];
        let worst = 0f64;
        for listed in listing.sounds.iter().step_by(step).take(sample) {
            let started = Instant::now();
            let fresh = measure_worker_sound_with(&listed.sound, rate, Reuse::Fresh);
            times[0] += started.elapsed();
            let warm = measure_worker_sound_with(&listed.sound, rate, Reuse::Warm);
            let started = Instant::now();
            let reactivated = measure_worker_sound_with(&listed.sound, rate, Reuse::Reactivate);
            times[1] += started.elapsed();
            let (Ok(fresh), Ok(warm), Ok(reactivated)) = (fresh, warm, reactivated) else {
                eprintln!("{}: not measurable", listed.name);
                continue;
            };
            measured += 1;
            let w = gap(axes(&fresh), axes(&warm));
            let r = gap(axes(&fresh), axes(&reactivated));
            let wr = gap(axes(&warm), axes(&reactivated));
            warm_beyond += usize::from(beyond(w));
            reactivated_beyond += usize::from(beyond(r));
            both += usize::from(beyond(w) && beyond(r));
            eprintln!(
                "{:<36} fresh/warm {w:.3?} fresh/reactivated {r:.3?} warm/reactivated {wr:.3?}",
                listed.name
            );
        }
        eprintln!(
            "{measured} presets: fresh {:.1?}, reactivated {:.1?}; beyond: warm {warm_beyond}, reactivated {reactivated_beyond}, both {both}",
            times[0], times[1]
        );
        let _ = worst;
    }
}
