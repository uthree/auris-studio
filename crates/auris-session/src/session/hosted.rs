//! Third-party CLAP plugins, and the awkward fact that a graph is rebuilt more often than a
//! plugin should be.
//!
//! # Why a slot owns two instances
//!
//! Everything else in a render graph is thrown away and built again on every structural edit —
//! add a track, change a solo, move a clip — because building a biquad costs nothing. A hosted
//! plugin is not like that. Surge XT reads its own data folder, publishes seven hundred and
//! seventy-five parameters and carries fifty kilobytes of patch; instantiating one per edit would
//! be felt.
//!
//! It cannot simply be kept, either. The graph *owns* its effects, and the graph the engine is
//! rendering does not come back until the audio thread has swapped it out and handed it down the
//! return channel — some time after the replacement has to exist. So at the moment a new graph is
//! built, the old effect is still out, and a CLAP plugin will not activate twice.
//!
//! Hence the pair. A slot keeps the instance whose effect is in the live graph and, once that
//! effect comes home, keeps *it* as the spare for next time. A burst of edits alternates between
//! two instances; a quiet session settles back to one. What is never paid is the cost of reading
//! the plugin off disk again.
//!
//! # What travels between them
//!
//! On a handover the outgoing instance is asked for its state and the incoming one is given it,
//! so anything the plugin changed about itself — a preset loaded from its own interface, a knob
//! turned there — survives. The document's parameter map is then applied on top, because that is
//! the half the user automated and it must win.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_clap::{ClapEffect, ClapError, ClapInstrument, ClapLibrary, ClapPlugin, ClapPluginInfo};
use auris_core::asset::AssetPath;
use auris_core::param::ParamDescriptor;
use auris_core::plugin::{Parameterized, PluginState, PrepareContext};
use auris_core::project::{EffectSlotId, Project, TrackId};
use auris_engine::{PlacedEffects, PlacedInstruments};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

/// Every hosted plugin this session has, and the files they came from.
#[derive(Default)]
pub(super) struct HostedPlugins {
    /// One entry per `.clap` file, however many slots use it. Loading the same file twice would
    /// be two copies of the plugin's code in the process.
    libraries: BTreeMap<PathBuf, ClapLibrary>,
    slots: BTreeMap<EffectSlotId, HostedSlot>,
    /// A track holds at most one instrument, so these are keyed by the track rather than by a
    /// slot of their own. Everything else about them is the same problem with the same answer.
    instruments: BTreeMap<TrackId, HostedSlot>,
}

/// One place in the project filled by a hosted plugin.
struct HostedSlot {
    file: PathBuf,
    clap_id: String,
    /// The instance whose rendering half is in the graph the engine holds.
    live: Option<ClapPlugin>,
    /// An instance with nothing out, ready to be activated for the next graph.
    spare: Option<ClapPlugin>,
}

impl HostedPlugins {
    /// Loads a `.clap` file and lists what is inside it, reusing an already-open one.
    ///
    /// # Safety
    ///
    /// Delegates to [`ClapLibrary::load`]: this maps third-party machine code into the process.
    /// The caller is asserting the user asked for this file.
    pub(super) unsafe fn catalog(&mut self, file: &Path) -> Result<Vec<ClapPluginInfo>, ClapError> {
        let library = match self.libraries.get(file) {
            Some(library) => library,
            None => {
                // SAFETY: delegated to this function's own contract.
                let library = unsafe { ClapLibrary::load(file) }?;
                self.libraries.entry(file.to_path_buf()).or_insert(library)
            }
        };
        library.plugins()
    }

    /// The parameters a hosted slot's plugin declares, or `None` when the slot has no live
    /// instance to ask.
    pub(super) fn parameters(&self, slot: EffectSlotId) -> Option<&[ParamDescriptor]> {
        self.slots.get(&slot)?.plugin().map(ClapPlugin::parameters)
    }

    /// The same, for a track's hosted instrument.
    pub(super) fn instrument_parameters(&self, track: TrackId) -> Option<&[ParamDescriptor]> {
        self.instruments
            .get(&track)?
            .plugin()
            .map(ClapPlugin::parameters)
    }

    /// What a hosted slot's plugin calls itself.
    pub(super) fn name(&self, slot: EffectSlotId) -> Option<&str> {
        self.slots.get(&slot)?.plugin().map(name_of)
    }

    /// The same, for a track's hosted instrument.
    pub(super) fn instrument_name(&self, track: TrackId) -> Option<&str> {
        self.instruments.get(&track)?.plugin().map(name_of)
    }

    /// The state a hosted slot's plugin is carrying, for the document to save.
    ///
    /// Asks the instance that is rendering, because it is the one a preset or an on-screen knob
    /// would have changed.
    pub(super) fn save_state(&mut self, slot: EffectSlotId) -> Option<Vec<u8>> {
        self.slots.get_mut(&slot)?.plugin_mut()?.save_state().ok()
    }

    /// The same, for a track's hosted instrument.
    pub(super) fn save_instrument_state(&mut self, track: TrackId) -> Option<Vec<u8>> {
        self.instruments
            .get_mut(&track)?
            .plugin_mut()?
            .save_state()
            .ok()
    }

    /// Forgets everything. Called when the document is replaced.
    pub(super) fn clear(&mut self) {
        self.slots.clear();
        self.instruments.clear();
        // The libraries stay: the same file is the same code whichever project is open, and
        // reading Surge XT's data folder again on every **File → New** would be felt.
    }

    /// Builds the effect for every hosted slot in `project`.
    ///
    /// Slots the project no longer has are dropped. A slot whose file or plugin cannot be loaded
    /// produces nothing, which leaves the graph to fill it with the bypassed placeholder it uses
    /// for any effect it cannot build — a project with a plugin that is not installed opens, with
    /// that one slot silent.
    pub(super) fn place(&mut self, project: &Project, prepare: &PrepareContext) -> PlacedEffects {
        let wanted = hosted_slots(project);
        self.slots.retain(|id, _| wanted.contains_key(id));

        let mut placed = PlacedEffects::new();
        for (id, request) in &wanted {
            let Some(file) = self.library_path(&request.file) else {
                continue;
            };
            let slot = fit(&mut self.slots, *id, &file, &request.clap_id);
            let Some(library) = self.libraries.get(&file) else {
                continue;
            };
            match slot.take_effect(library, request.state, prepare) {
                Ok(effect) => {
                    placed.insert(*id, Box::new(effect) as Box<dyn auris_core::plugin::Effect>);
                }
                Err(error) => {
                    log::warn!(
                        "`{}` keeps its slot but stays silent: {error}",
                        request.clap_id
                    );
                }
            }
        }
        placed
    }

    /// Builds the instrument for every track in `project` whose sound is a hosted plugin.
    ///
    /// The same contract as [`place`](Self::place): tracks that no longer name a plugin are
    /// dropped, and a plugin that cannot be built leaves the track to the graph's silent
    /// stand-in rather than failing the rebuild.
    pub(super) fn place_instruments(
        &mut self,
        project: &Project,
        prepare: &PrepareContext,
    ) -> PlacedInstruments {
        let wanted = hosted_instruments(project);
        self.instruments.retain(|id, _| wanted.contains_key(id));

        let mut placed = PlacedInstruments::new();
        for (track, request) in &wanted {
            let Some(file) = self.library_path(&request.file) else {
                continue;
            };
            let slot = fit(&mut self.instruments, *track, &file, &request.clap_id);
            let Some(library) = self.libraries.get(&file) else {
                continue;
            };
            match slot.take_instrument(library, request.state, prepare) {
                Ok(instrument) => {
                    placed.insert(
                        *track,
                        Box::new(instrument) as Box<dyn auris_core::plugin::Instrument>,
                    );
                }
                Err(error) => {
                    log::warn!(
                        "`{}` keeps its track but stays silent: {error}",
                        request.clap_id
                    );
                }
            }
        }
        placed
    }

    /// Makes sure a file is open, returning the key it is filed under.
    fn library_path(&mut self, file: &Path) -> Option<PathBuf> {
        if self.libraries.contains_key(file) {
            return Some(file.to_path_buf());
        }
        // SAFETY: the file is named by the document, which named it because the user chose it.
        // Nothing safer is available: hosting a plugin means running its code.
        match unsafe { ClapLibrary::load(file) } {
            Ok(library) => {
                self.libraries.insert(file.to_path_buf(), library);
                Some(file.to_path_buf())
            }
            Err(error) => {
                log::warn!("cannot load `{}`: {error}", file.display());
                None
            }
        }
    }
}

impl HostedSlot {
    /// Whichever instance can answer a question about the plugin.
    fn plugin(&self) -> Option<&ClapPlugin> {
        self.live.as_ref().or(self.spare.as_ref())
    }

    /// The same, to be asked something that changes it.
    fn plugin_mut(&mut self) -> Option<&mut ClapPlugin> {
        self.live.as_mut().or(self.spare.as_mut())
    }

    /// Produces the effect for the next graph, reusing an instance when one is free.
    fn take_effect(
        &mut self,
        library: &ClapLibrary,
        state: &PluginState,
        prepare: &PrepareContext,
    ) -> Result<ClapEffect, ClapError> {
        let mut plugin = self.incoming(library, state)?;
        let mut effect = plugin.activate(prepare)?;
        // The document's parameter map last, so a value the user automated wins over the one
        // baked into the patch.
        effect.load_state(state);
        self.settle(plugin);
        Ok(effect)
    }

    /// The same, for a track's instrument.
    fn take_instrument(
        &mut self,
        library: &ClapLibrary,
        state: &PluginState,
        prepare: &PrepareContext,
    ) -> Result<ClapInstrument, ClapError> {
        let mut plugin = self.incoming(library, state)?;
        let mut instrument = plugin.activate_instrument(prepare)?;
        instrument.load_state(state);
        self.settle(plugin);
        Ok(instrument)
    }

    /// The instance to activate next, carrying whatever state it should already have.
    fn incoming(
        &mut self,
        library: &ClapLibrary,
        state: &PluginState,
    ) -> Result<ClapPlugin, ClapError> {
        self.reclaim();

        let mut plugin = match self.spare.take() {
            Some(plugin) => plugin,
            None => library.instantiate(&self.clap_id)?,
        };

        // Where the incoming instance's state comes from: the outgoing one if there is one, so a
        // preset the user loaded inside the plugin survives a rebuild; otherwise the document,
        // which is the case on the first build after opening a project.
        let bytes = match self.live.as_mut() {
            Some(outgoing) => outgoing.save_state().ok(),
            None => state.hosted_bytes(),
        };
        if let Some(bytes) = bytes
            && let Err(error) = plugin.load_state(&bytes)
        {
            log::warn!("`{}` could not take its state back: {error}", self.clap_id);
        }
        Ok(plugin)
    }

    /// Files the newly activated instance as the live one.
    ///
    /// The outgoing instance is not dropped: its rendering half is still in the graph the engine
    /// has not let go of, and it is the one `reclaim` will pick up next time.
    fn settle(&mut self, plugin: ClapPlugin) {
        if let Some(outgoing) = self.live.replace(plugin) {
            self.spare = Some(outgoing);
        }
    }

    /// Takes back whichever instances have had their effects dropped.
    ///
    /// Keeps at most one spare, and prefers the most recently used, which is the one carrying the
    /// newest state. A slot that is rebuilt rarely ends up back at a single instance.
    fn reclaim(&mut self) {
        if self.live.as_mut().is_some_and(ClapPlugin::release) {
            self.spare = self.live.take();
        }
        // A spare with an effect still out is not a spare: handing it to `activate` would be told
        // the plugin is already active, and the slot would go silent for no reason a user could
        // see. Letting go of it is safe — the effect keeps the plugin itself alive until whatever
        // is holding it is dropped — and its state was copied forward when it stopped being live.
        //
        // This is not the two-instance case; it is the three-instance one, which an export
        // running while the transport plays produces.
        if self.spare.as_mut().is_some_and(|spare| !spare.release()) {
            self.spare = None;
        }
    }
}

/// What one hosted slot asks the session for.
struct HostedRequest<'a> {
    file: PathBuf,
    clap_id: String,
    state: &'a PluginState,
}

/// What a plugin calls itself.
fn name_of(plugin: &ClapPlugin) -> &str {
    plugin.info().name.as_str()
}

/// The slot filed under `key`, made to match what the document now asks for.
///
/// A slot whose plugin was swapped for another is a different slot in all but its key, and starts
/// again with no instances: keeping them would hand the next graph a Surge XT where the document
/// says Vital.
fn fit<'a, K: Ord + Copy>(
    slots: &'a mut BTreeMap<K, HostedSlot>,
    key: K,
    file: &Path,
    clap_id: &str,
) -> &'a mut HostedSlot {
    let fresh = || HostedSlot {
        file: file.to_path_buf(),
        clap_id: clap_id.to_string(),
        live: None,
        spare: None,
    };
    let slot = slots.entry(key).or_insert_with(fresh);
    if slot.clap_id != clap_id || slot.file != file {
        *slot = fresh();
    }
    slot
}

/// Reads the hosted slots out of a project.
///
/// A slot is hosted when it names a file. Its `effect_id` carries the `clap:` prefix that keeps
/// hosted ids out of the registry's namespace, and what the plugin itself answers to is the rest.
fn hosted_slots(project: &Project) -> BTreeMap<EffectSlotId, HostedRequest<'_>> {
    let strips = std::iter::once(&project.master).chain(project.tracks.iter().map(|t| &t.mixer));

    strips
        .flat_map(|strip| strip.effects.iter())
        .filter_map(|slot| {
            let file = slot.file.as_ref()?;
            // A plugin is always `External`, so resolving it needs no project folder — but going
            // through `resolve` rather than unwrapping the variant keeps the one rule in one
            // place, and correctly gives up on an `Inside` reference nobody should have written.
            let file = file.resolve(None)?;
            Some((
                slot.id,
                HostedRequest {
                    file,
                    clap_id: clap_id_of(&slot.effect_id).to_string(),
                    state: &slot.state,
                },
            ))
        })
        .collect()
}

/// Reads the tracks whose instrument is a hosted plugin.
///
/// The same rule as [`hosted_slots`]: naming a file is what makes it hosted.
fn hosted_instruments(project: &Project) -> BTreeMap<TrackId, HostedRequest<'_>> {
    project
        .tracks
        .iter()
        .filter_map(|track| {
            let inner = track.kind.as_instrument()?;
            let file = inner.file.as_ref()?.resolve(None)?;
            Some((
                track.id,
                HostedRequest {
                    file,
                    clap_id: clap_id_of(&inner.instrument_id).to_string(),
                    state: &inner.instrument_state,
                },
            ))
        })
        .collect()
}

/// Strips the namespace prefix an Auris id wears, leaving what the plugin calls itself.
fn clap_id_of(effect_id: &str) -> &str {
    effect_id.strip_prefix("clap:").unwrap_or(effect_id)
}

/// Where CLAP plugins are installed, as the specification lays them out.
///
/// The list is per platform and is not configurable *here*: a user with plugins somewhere else
/// points at the file, which is what [`Session::hosted_plugins_in`] takes.
pub fn clap_search_paths() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);

    match cfg!(target_os = "windows") {
        true => [
            var("CommonProgramFiles").map(|path| path.join("CLAP")),
            var("LOCALAPPDATA").map(|path| path.join("Programs").join("Common").join("CLAP")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        false => [
            Some(PathBuf::from("/Library/Audio/Plug-Ins/CLAP")),
            var("HOME").map(|home| home.join("Library/Audio/Plug-Ins/CLAP")),
            var("HOME").map(|home| home.join(".clap")),
            Some(PathBuf::from("/usr/lib/clap")),
        ]
        .into_iter()
        .flatten()
        .collect(),
    }
}

/// Every `.clap` under `directory`, however deep.
///
/// On macOS a `.clap` is a bundle *directory*, so the extension is tested before the file type —
/// the other order would walk into the bundle and find nothing.
fn collect_clap_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "clap")
        {
            found.push(path);
        } else if path.is_dir() {
            collect_clap_files(&path, found);
        }
    }
}

impl Session {
    /// Every `.clap` file installed in the usual places, in a stable order.
    ///
    /// Only the files: opening them is what [`Self::hosted_plugins_in`] does, and a browser that
    /// loaded every plugin on the machine to draw a list would run their code to do it.
    pub fn installed_clap_files(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for root in clap_search_paths() {
            collect_clap_files(&root, &mut found);
        }
        found.sort();
        found.dedup();
        found
    }

    /// Opens a `.clap` file and lists the plugins inside it.
    ///
    /// This runs third-party code — loading a plugin is running it — so it happens when somebody
    /// asks for this file and not before. The file stays open afterwards, because a browser that
    /// closed it would reopen it the moment the plugin was used.
    pub fn hosted_plugins_in(&mut self, file: &Path) -> Result<Vec<ClapPluginInfo>, SessionError> {
        // SAFETY: the caller reached this because a user chose the file. Nothing safer exists:
        // there is no way to read a CLAP plugin's contents without loading its binary.
        Ok(unsafe { self.hosted.catalog(file) }?)
    }

    /// Adds a hosted plugin to a track's chain, or to the master bus when `track` is `None`.
    ///
    /// The plugin is instantiated on the next graph build, not here — which is also when a plugin
    /// that has gone missing since it was listed reports itself, by leaving its slot silent.
    pub fn add_hosted_effect(
        &mut self,
        track: Option<TrackId>,
        file: &Path,
        clap_id: &str,
    ) -> Result<EffectSlotId, SessionError> {
        if let Some(id) = track {
            self.require_track(id)?;
        }
        // Checked before anything is recorded: a plugin that is not in the file must not cost an
        // undo step and a slot that can never be filled.
        let known = self
            .hosted_plugins_in(file)?
            .into_iter()
            .find(|info| info.clap_id == clap_id)
            .ok_or_else(|| SessionError::UnknownPlugin(clap_id.to_string()))?;

        self.record(Edit::AddEffect);
        let slot = self
            .project
            .add_hosted_effect(track, known.auris_id(), AssetPath::external(file))
            .ok_or_else(|| SessionError::UnknownTrack(track.map_or(0, |t| t.0)))?;
        self.invalidate_graph();
        Ok(slot)
    }

    /// Points a track's instrument at a plugin hosted from a file.
    ///
    /// The clips stay where they are — this changes what plays the part, not the part. The plugin
    /// itself is instantiated on the next graph build, not here.
    pub fn set_hosted_instrument(
        &mut self,
        track: TrackId,
        file: &Path,
        clap_id: &str,
    ) -> Result<(), SessionError> {
        self.require_track(track)?;
        // Checked before anything is recorded, so a plugin that is not in the file costs neither
        // an undo step nor a track that can no longer be heard.
        let known = self
            .hosted_plugins_in(file)?
            .into_iter()
            .find(|info| info.clap_id == clap_id)
            .ok_or_else(|| SessionError::UnknownPlugin(clap_id.to_string()))?;

        self.record(Edit::ChangeInstrument);
        if !self
            .project
            .set_hosted_instrument(track, known.auris_id(), AssetPath::external(file))
        {
            return Err(SessionError::UnknownTrack(track.0));
        }
        self.invalidate_graph();
        Ok(())
    }

    /// The parameters a hosted slot's plugin declares.
    ///
    /// Empty until the plugin has been built, which is the first graph rebuild after it was
    /// added — and empty for good if it could not be loaded.
    pub fn hosted_parameters(&self, slot: EffectSlotId) -> &[ParamDescriptor] {
        self.hosted.parameters(slot).unwrap_or(&[])
    }

    /// The parameters a track's hosted instrument declares.
    pub fn hosted_instrument_parameters(&self, track: TrackId) -> &[ParamDescriptor] {
        self.hosted.instrument_parameters(track).unwrap_or(&[])
    }

    /// What a track's hosted instrument calls itself, or `None` when it is a built-in.
    pub fn hosted_instrument_name(&self, track: TrackId) -> Option<&str> {
        self.hosted.instrument_name(track)
    }

    /// The parameters of whatever plays a track, hosted or built in.
    ///
    /// The instrument counterpart of [`Self::effect_descriptors`], and the same reasoning: the
    /// registry cannot answer for a plugin it did not build, so the track is the thing to ask
    /// about.
    pub fn instrument_descriptors(&mut self, track: TrackId) -> Arc<Vec<ParamDescriptor>> {
        let hosted = self.hosted_instrument_parameters(track);
        if !hosted.is_empty() {
            return Arc::new(hosted.to_vec());
        }
        let Some(id) = self
            .project
            .track(track)
            .and_then(|track| track.kind.as_instrument())
            .map(|inner| inner.instrument_id.clone())
        else {
            return Arc::new(Vec::new());
        };
        self.param_descriptors(&id)
    }

    /// What a hosted slot's plugin calls itself, or `None` for a slot that is not hosted.
    ///
    /// A hosted plugin has no registry entry, so the id a frontend would otherwise show is a
    /// reverse-DNS string somebody else chose — `clap:org.surge-synth-team.surge-xt-fx` where a
    /// built-in would have said "Reverb".
    pub fn hosted_name(&self, slot: EffectSlotId) -> Option<&str> {
        self.hosted.name(slot)
    }

    /// The parameters of whatever is in an effect slot, hosted or built in.
    ///
    /// The registry can only be asked by plugin id, which for a hosted plugin answers nothing —
    /// and could not answer correctly anyway, since two slots may hold the same plugin from two
    /// different files. So the slot is the thing to ask about.
    pub fn effect_descriptors(&mut self, slot: EffectSlotId) -> Arc<Vec<ParamDescriptor>> {
        let hosted = self.hosted_parameters(slot);
        if !hosted.is_empty() {
            return Arc::new(hosted.to_vec());
        }
        let Some(effect_id) = std::iter::once(&self.project.master)
            .chain(self.project.tracks.iter().map(|track| &track.mixer))
            .flat_map(|strip| strip.effects.iter())
            .find(|existing| existing.id == slot)
            .map(|existing| existing.effect_id.clone())
        else {
            return Arc::new(Vec::new());
        };
        self.param_descriptors(&effect_id)
    }

    /// Copies every hosted plugin's own state into the document.
    ///
    /// Called before the document is written. A CLAP plugin's state is not a thing the session
    /// can keep in step as it changes — the plugin changes it whenever it likes, and says so only
    /// by setting a flag — so it is collected at the one moment it has to be right.
    pub(super) fn collect_hosted_state(&mut self) {
        let slots: Vec<EffectSlotId> = hosted_slots(&self.project).into_keys().collect();
        for id in slots {
            let Some(bytes) = self.hosted.save_state(id) else {
                continue;
            };
            for strip in std::iter::once(&mut self.project.master)
                .chain(self.project.tracks.iter_mut().map(|track| &mut track.mixer))
            {
                if let Some(slot) = strip.effects.iter_mut().find(|slot| slot.id == id) {
                    slot.state.set_hosted_bytes(&bytes);
                }
            }
        }

        let tracks: Vec<TrackId> = hosted_instruments(&self.project).into_keys().collect();
        for id in tracks {
            let Some(bytes) = self.hosted.save_instrument_state(id) else {
                continue;
            };
            if let Some(inner) = self
                .project
                .track_mut(id)
                .and_then(|track| track.kind.as_instrument_mut())
            {
                inner.instrument_state.set_hosted_bytes(&bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_clap::testkit::{FIXTURE_ID, TONE_ID, fixture_library, instrument_library};
    use auris_core::param::ParamId;
    use auris_core::plugin::{Effect, ProcessContext};
    use auris_core::time::Ticks;
    use auris_core::{AssetPath, AudioBuffer};

    fn prepare() -> PrepareContext {
        PrepareContext::new(48_000.0, 64, 2)
    }

    fn slot() -> HostedSlot {
        HostedSlot {
            file: PathBuf::from("/auris/testkit/test-gain.clap"),
            clap_id: FIXTURE_ID.to_string(),
            live: None,
            spare: None,
        }
    }

    fn render(effect: &mut ClapEffect, level: f32) -> f32 {
        let mut buffer = AudioBuffer::stereo(8, 48_000.0);
        for channel in 0..buffer.channel_count() {
            buffer.channel_mut(channel).fill(level);
        }
        effect.process(
            &mut buffer,
            &ProcessContext::realtime(48_000.0, 8, 0, 120.0, true),
        );
        buffer.channel(0)[0]
    }

    #[test]
    fn an_id_keeps_its_namespace_out_of_the_plugins_way() {
        assert_eq!(clap_id_of("clap:com.u-he.diva"), "com.u-he.diva");
        // A plugin whose own id starts with the prefix is not double-stripped.
        assert_eq!(clap_id_of("clap:clap:odd"), "clap:odd");
        assert_eq!(clap_id_of("auris.dsp.gain"), "auris.dsp.gain");
    }

    #[test]
    fn only_a_slot_that_names_a_file_is_hosted() {
        let mut project = Project::new("Hosted", 48_000.0);
        let track = project.add_instrument_track("Lead", "auris.synth.pulse");
        let built_in = project.add_effect(Some(track), "auris.dsp.gain").unwrap();
        let hosted = project
            .add_hosted_effect(
                Some(track),
                "clap:studio.auris.test.gain",
                AssetPath::external("/plugins/test.clap"),
            )
            .unwrap();

        let found = hosted_slots(&project);
        assert!(!found.contains_key(&built_in));
        assert_eq!(found.len(), 1);
        assert_eq!(found[&hosted].clap_id, "studio.auris.test.gain");
    }

    #[test]
    fn a_rebuild_while_the_old_effect_is_still_out_makes_a_second_instance() {
        // The case the whole pair exists for: the engine has not handed the previous graph back,
        // so the plugin that is rendering cannot be activated again.
        let library = fixture_library();
        let mut slot = slot();
        let state = PluginState::empty();

        let first = slot
            .take_effect(&library, &state, &prepare())
            .expect("first");
        assert!(slot.live.is_some());
        assert!(slot.spare.is_none(), "nothing has been freed yet");

        let second = slot
            .take_effect(&library, &state, &prepare())
            .expect("second");
        assert!(
            slot.spare.is_some(),
            "the outgoing instance is kept, not dropped, because its effect is still rendering"
        );

        drop(first);
        drop(second);
    }

    #[test]
    fn a_slot_settles_back_to_one_instance_once_the_graph_comes_home() {
        let library = fixture_library();
        let mut slot = slot();
        let state = PluginState::empty();

        let first = slot
            .take_effect(&library, &state, &prepare())
            .expect("first");
        drop(first); // the engine returned the graph and it was dropped

        let second = slot
            .take_effect(&library, &state, &prepare())
            .expect("second");
        assert!(
            slot.spare.is_none(),
            "a freed instance is reused rather than added to"
        );
        drop(second);
    }

    #[test]
    fn a_handover_carries_the_plugins_own_state_across() {
        // A preset loaded inside the plugin is not in the document, and a rebuild must not lose
        // it. The fixture's state *is* its gain, so this is visible in the audio.
        let library = fixture_library();
        let mut slot = slot();
        let state = PluginState::empty();

        let mut first = slot
            .take_effect(&library, &state, &prepare())
            .expect("first");
        first.set_param(ParamId(0), 0.5);
        assert_eq!(render(&mut first, 1.0), 0.5);
        drop(first);

        let mut second = slot
            .take_effect(&library, &state, &prepare())
            .expect("second");
        assert_eq!(
            render(&mut second, 1.0),
            0.5,
            "the new instance must start where the old one left off"
        );
    }

    #[test]
    fn the_document_wins_over_the_patch() {
        // The other direction: what the user automated is in the document's parameter map, and a
        // patch that disagrees must not overwrite it.
        let library = fixture_library();
        let mut slot = slot();

        let mut patched = PluginState::empty();
        patched.set_hosted_bytes(&0.25f32.to_le_bytes());
        patched.params.insert("clap.4242".into(), 2.0);

        let mut effect = slot
            .take_effect(&library, &patched, &prepare())
            .expect("built");
        assert_eq!(render(&mut effect, 0.5), 1.0, "gain 2.0 from the document");
    }

    #[test]
    fn a_slot_that_leaves_the_project_is_forgotten() {
        let mut hosted = HostedPlugins::default();
        hosted.libraries.insert(
            PathBuf::from("/auris/testkit/test-gain.clap"),
            fixture_library(),
        );

        let mut project = Project::new("Hosted", 48_000.0);
        let track = project.add_instrument_track("Lead", "auris.synth.pulse");
        let slot = project
            .add_hosted_effect(
                Some(track),
                "clap:studio.auris.test.gain",
                AssetPath::external("/auris/testkit/test-gain.clap"),
            )
            .unwrap();

        let placed = hosted.place(&project, &prepare());
        assert_eq!(placed.len(), 1);
        assert!(hosted.slots.contains_key(&slot));
        drop(placed);

        project.remove_effect(slot);
        let placed = hosted.place(&project, &prepare());
        assert!(placed.is_empty());
        assert!(
            !hosted.slots.contains_key(&slot),
            "an instance nobody can reach is an instance nobody can free"
        );
    }

    /// A session with the fixture already open, standing in for a `.clap` on disk.
    fn session_with_fixture() -> (super::Session, PathBuf) {
        let mut session = super::super::fixtures::session();
        let file = PathBuf::from("/auris/testkit/test-gain.clap");
        session
            .hosted
            .libraries
            .insert(file.clone(), fixture_library());
        (session, file)
    }

    /// The same, with the instrument fixture open as a second file.
    fn session_with_instrument() -> (super::Session, PathBuf) {
        let mut session = super::super::fixtures::session();
        let file = PathBuf::from("/auris/testkit/test-tone.clap");
        session
            .hosted
            .libraries
            .insert(file.clone(), instrument_library());
        (session, file)
    }

    #[test]
    fn a_hosted_instrument_replaces_what_plays_a_track_and_not_the_part() {
        let (mut session, file) = session_with_instrument();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(
                clip,
                auris_core::project::Note::new(60, Ticks::ZERO, Ticks::QUARTER),
            )
            .unwrap();

        session
            .set_hosted_instrument(track, &file, TONE_ID)
            .expect("the fixture is in the library");

        let inner = session
            .project
            .track(track)
            .and_then(|track| track.kind.as_instrument())
            .expect("still an instrument track");
        assert_eq!(inner.instrument_id, "clap:studio.auris.test.tone");
        assert!(inner.is_hosted());
        assert_eq!(inner.clips.len(), 1, "the part is not the instrument");

        // Non-empty is proof the plugin was really built and placed.
        assert_eq!(session.hosted_instrument_parameters(track).len(), 3);
        assert_eq!(session.hosted_instrument_name(track), Some("Test Tone"));
        assert_eq!(
            session.instrument_descriptors(track).len(),
            3,
            "and the plugin editor finds them by asking about the track"
        );

        // And a parameter of it can be described, which is what makes its sliders draggable
        // rather than merely visible.
        let target = crate::param::ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        let descriptor = session
            .descriptor_for(target)
            .expect("a hosted instrument parameter must be describable");
        assert_eq!(descriptor.name, "Level");
        session.set_param(target, 0.25);
        assert_eq!(session.param_value(target, &descriptor), 0.25);
    }

    #[test]
    fn a_hosted_instrument_actually_plays_the_notes() {
        // The whole point, end to end: a note in the document, through the session's own graph
        // build, into a third-party plugin, and back out as samples.
        let (mut session, file) = session_with_instrument();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(
                clip,
                auris_core::project::Note::new(60, Ticks::ZERO, Ticks::from_beats(2.0)),
            )
            .unwrap();
        session
            .set_hosted_instrument(track, &file, TONE_ID)
            .unwrap();

        let rendered = session
            .render_job()
            .render(&auris_engine::OfflineOptions::whole_project(), &mut |_| {})
            .expect("render");
        assert!(
            rendered.peak() > 0.5,
            "the fixture holds the note's velocity while it sounds, so silence means the notes \
             never arrived"
        );
    }

    #[test]
    fn a_track_that_goes_back_to_a_built_in_lets_its_plugin_go() {
        let (mut session, file) = session_with_instrument();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .set_hosted_instrument(track, &file, TONE_ID)
            .unwrap();
        assert!(session.hosted.instruments.contains_key(&track));

        session
            .set_track_instrument(track, "auris.synth.chiptune")
            .unwrap();
        session.rebuild_graph();
        assert!(
            !session.hosted.instruments.contains_key(&track),
            "an instance nobody can reach is an instance nobody can free"
        );
        assert_eq!(session.hosted_instrument_name(track), None);
    }

    #[test]
    fn saving_captures_what_a_hosted_instrument_did_to_itself() {
        let (mut session, file) = session_with_instrument();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .set_hosted_instrument(track, &file, TONE_ID)
            .unwrap();

        session.collect_hosted_state();
        let stored = session
            .project
            .track(track)
            .and_then(|track| track.kind.as_instrument())
            .expect("an instrument track")
            .instrument_state
            .hosted_bytes()
            .expect("a hosted track must carry its plugin's state after a save");
        assert_eq!(
            stored,
            1.0f32.to_le_bytes(),
            "the fixture's state is its level, which starts at unity"
        );
    }

    #[test]
    fn a_hosted_effect_goes_into_the_chain_and_is_really_instantiated() {
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();

        let slot = session
            .add_hosted_effect(Some(track), &file, FIXTURE_ID)
            .expect("the fixture is in the library");

        let stored = &session.project.track(track).unwrap().mixer.effects[0];
        assert_eq!(stored.id, slot);
        assert_eq!(stored.effect_id, "clap:studio.auris.test.gain");
        assert_eq!(stored.file, Some(AssetPath::external(&file)));
        assert!(stored.is_hosted());

        // Empty until the plugin exists, so a non-empty list is proof it was built and placed.
        let params = session.hosted_parameters(slot);
        assert_eq!(
            params.len(),
            4,
            "the fixture's gain, its port report, its tick count and its window report"
        );
        assert_eq!(params[0].key, "clap.4242");

        // And the parameter panel finds it by the same route every other parameter takes.
        let descriptor = session
            .descriptor_for(crate::param::ParamTarget::Effect {
                track: Some(track),
                slot,
                param: ParamId(0),
            })
            .expect("a hosted parameter must be describable");
        assert_eq!(descriptor.name, "Gain");
    }

    #[test]
    fn a_hosted_parameter_can_actually_be_moved() {
        // Describing one and being able to *write* one were two different lookups, and only the
        // first knew about hosted plugins. The second went to the registry for the key to store
        // the value under, got nothing, and returned — so the window drew thirteen sliders that
        // could not be dragged.
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let slot = session
            .add_hosted_effect(Some(track), &file, FIXTURE_ID)
            .unwrap();
        let target = crate::param::ParamTarget::Effect {
            track: Some(track),
            slot,
            param: ParamId(0),
        };
        let descriptor = session.descriptor_for(target).unwrap();

        session.set_param(target, 0.25);
        assert_eq!(session.param_value(target, &descriptor), 0.25);
        assert_eq!(
            session.project.track(track).unwrap().mixer.effects[0]
                .state
                .params
                .get("clap.4242"),
            Some(&0.25),
            "the value has to land under the plugin's own key, or a save loses it"
        );
    }

    #[test]
    fn a_plugin_the_file_does_not_hold_costs_no_undo_step() {
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let before = super::super::fixtures::undo_depth(&mut session);

        let error = session
            .add_hosted_effect(Some(track), &file, "com.example.nothing")
            .unwrap_err();
        assert!(error.to_string().contains("com.example.nothing"));

        assert!(
            session
                .project
                .track(track)
                .unwrap()
                .mixer
                .effects
                .is_empty()
        );
        assert_eq!(super::super::fixtures::undo_depth(&mut session), before);
    }

    #[test]
    fn saving_captures_what_the_plugin_did_to_itself() {
        // The document's parameter map is kept in step as the user edits. The plugin's own state
        // is not, and cannot be — so a save has to go and ask for it.
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let slot = session
            .add_hosted_effect(Some(track), &file, FIXTURE_ID)
            .unwrap();

        session.collect_hosted_state();
        let stored = session.project.track(track).unwrap().mixer.effects[0]
            .state
            .hosted_bytes()
            .expect("a hosted slot must carry its plugin's state after a save");
        assert_eq!(
            stored,
            1.0f32.to_le_bytes(),
            "the fixture's state is its gain, which starts at unity"
        );
        assert_eq!(session.hosted_parameters(slot).len(), 4);
    }

    #[test]
    fn a_plugin_that_will_not_load_leaves_its_slot_empty_rather_than_failing() {
        let mut hosted = HostedPlugins::default();
        let mut project = Project::new("Hosted", 48_000.0);
        let track = project.add_instrument_track("Lead", "auris.synth.pulse");
        project.add_hosted_effect(
            Some(track),
            "clap:com.example.absent",
            AssetPath::external("/nowhere/absent.clap"),
        );

        assert!(
            hosted.place(&project, &prepare()).is_empty(),
            "the graph fills an empty slot with its own placeholder"
        );
    }
}
