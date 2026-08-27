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
//!
//! # And what happens to the window
//!
//! A plugin's own window belongs to an *instance*; wanting it open is a fact about the *slot*. So
//! the slot keeps the wish and puts the window wherever the answer currently is — on the instance
//! [`HostedSlot::plugin_mut`] returns, and on no other. Two views of one slot that disagree, a
//! window drawn by one instance beside a parameter panel reading another, is the thing this
//! prevents; a window that moves on the rare rebuild which really does swap instances is what it
//! costs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use auris_clap::{
    ClapEffect, ClapError, ClapInstrument, ClapLibrary, ClapPlugin, ClapPluginInfo, RawWindowHandle,
};
use auris_core::asset::AssetPath;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{Effect, Parameterized, PluginState, PrepareContext};
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
    /// The window every plugin window is told to float above.
    ///
    /// Given by the frontend once, because a plugin window that has to be moved between instances
    /// needs it again at a moment nobody is asking for anything. The session cannot check that it
    /// is still a window; what makes it safe is that it names the application's own, which outlives
    /// the session — and dropping the session closes every plugin window before it goes.
    parent: Option<RawWindowHandle>,
    /// Instances let go of while their rendering halves were still out.
    ///
    /// Dropping a [`ClapPlugin`] whose effect is still in a graph does not free it — clack's
    /// `PluginInstance::Drop` deliberately *leaks* rather than hand ownership to the audio
    /// thread — so a slot on its way out parks its instances here instead, and
    /// [`Self::sweep_retired`] drops each one once its effect has come home down the engine's
    /// return channel and been dropped there.
    retiring: Vec<ClapPlugin>,
}

/// Which hosted plugin's own window is meant.
///
/// The two live in different maps, because a track holds at most one instrument and a chain holds
/// any number of effects. To everything about windows they are the same thing, so this is what a
/// frontend passes rather than picking between two near-identical methods.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PluginWindow {
    /// The window of a plugin in an effect chain.
    Effect(EffectSlotId),
    /// The window of the plugin that plays a track.
    Instrument(TrackId),
}

/// One place in the project filled by a hosted plugin.
struct HostedSlot {
    file: PathBuf,
    clap_id: String,
    /// The instance whose rendering half is in the graph the engine holds.
    live: Option<ClapPlugin>,
    /// An instance with nothing out, ready to be activated for the next graph.
    spare: Option<ClapPlugin>,
    /// Whether the user wants this plugin's own window on screen.
    ///
    /// A wish about the slot, held here rather than read off an instance, because the instance a
    /// window is drawn by can change under it and the wish must not.
    editor: bool,
    /// Whether the plugin declared a port for a key, as its last activation reported.
    ///
    /// Recorded when the effect is built rather than asked for on demand, because asking a
    /// `ClapPlugin` about its ports is `&mut` — it reads them back off the plugin — and a
    /// frontend deciding whether to draw a source picker is not.
    sidechain: bool,
    /// What the plugin last said its parameters were, by [`ParamId`].
    ///
    /// Read from the plugin rather than from the document, because for a hosted plugin the
    /// document knows only what Auris itself has written: a patch loaded inside the plugin, or a
    /// knob turned in its own window, happens somewhere the session never sees. Cached rather than
    /// asked at the moment of drawing because asking is `&mut` — the plugin may compute a value —
    /// and drawing is not.
    values: Vec<Option<f32>>,
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

    /// Whether a hosted slot's plugin declared a port for a key.
    ///
    /// `false` for a slot that is not hosted and for one whose plugin has never been built, which
    /// is the same answer either way: nothing to offer a source to.
    pub(super) fn wants_sidechain(&self, slot: EffectSlotId) -> bool {
        self.slots.get(&slot).is_some_and(|slot| slot.sidechain)
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

    /// Forgets every slot. Called when a *different* document takes over — see
    /// [`Session::open`]; an undo restoring a variant of the same document keeps its slots.
    pub(super) fn clear(&mut self) {
        for slot in std::mem::take(&mut self.slots).into_values() {
            slot.retire_into(&mut self.retiring);
        }
        for slot in std::mem::take(&mut self.instruments).into_values() {
            slot.retire_into(&mut self.retiring);
        }
        self.sweep_retired();
        // The libraries stay: the same file is the same code whichever project is open, and
        // reading Surge XT's data folder again on every **File → New** would be felt.
    }

    /// Drops every retired instance whose rendering half has come home, keeping the rest parked.
    ///
    /// [`ClapPlugin::release`] answers about the state, so an instance whose effect is still in
    /// a graph somewhere simply waits for a later sweep — every tick takes one, and so does
    /// every rebuild.
    pub(super) fn sweep_retired(&mut self) {
        self.retiring.retain_mut(|plugin| !plugin.release());
    }

    /// Builds the effect for every hosted slot in `project`.
    ///
    /// Slots the project no longer has are dropped. A slot whose file or plugin cannot be loaded
    /// produces nothing, which leaves the graph to fill it with the bypassed placeholder it uses
    /// for any effect it cannot build — a project with a plugin that is not installed opens, with
    /// that one slot silent.
    pub(super) fn place(&mut self, project: &Project, prepare: &PrepareContext) -> PlacedEffects {
        self.sweep_retired();
        let wanted = hosted_slots(project);
        let leaving: Vec<EffectSlotId> = self
            .slots
            .keys()
            .filter(|id| !wanted.contains_key(id))
            .copied()
            .collect();
        for id in leaving {
            if let Some(slot) = self.slots.remove(&id) {
                // Not dropped: the slot's live effect may still be in the graph the engine
                // holds, and letting go of the instance now would leak it — see `retiring`.
                slot.retire_into(&mut self.retiring);
            }
        }

        let mut placed = PlacedEffects::new();
        for (id, request) in &wanted {
            let Some(file) = self.library_path(&request.file) else {
                continue;
            };
            let slot = fit(
                &mut self.slots,
                *id,
                &file,
                &request.clap_id,
                &mut self.retiring,
            );
            let Some(library) = self.libraries.get(&file) else {
                continue;
            };
            match slot.take_effect(library, request.state, prepare, &mut self.retiring) {
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
        self.settle_editors();
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
        let leaving: Vec<TrackId> = self
            .instruments
            .keys()
            .filter(|id| !wanted.contains_key(id))
            .copied()
            .collect();
        for id in leaving {
            if let Some(slot) = self.instruments.remove(&id) {
                slot.retire_into(&mut self.retiring);
            }
        }

        let mut placed = PlacedInstruments::new();
        for (track, request) in &wanted {
            let Some(file) = self.library_path(&request.file) else {
                continue;
            };
            let slot = fit(
                &mut self.instruments,
                *track,
                &file,
                &request.clap_id,
                &mut self.retiring,
            );
            let Some(library) = self.libraries.get(&file) else {
                continue;
            };
            match slot.take_instrument(library, request.state, prepare, &mut self.retiring) {
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
        self.settle_editors();
        placed
    }

    /// The slot a window belongs to, whichever map it lives in.
    fn window_slot(&mut self, which: PluginWindow) -> Option<&mut HostedSlot> {
        match which {
            PluginWindow::Effect(id) => self.slots.get_mut(&id),
            PluginWindow::Instrument(id) => self.instruments.get_mut(&id),
        }
    }

    /// Asks a hosted plugin what its parameters are now, for the panel about to draw them.
    pub(super) fn refresh_values(&mut self, which: PluginWindow) {
        if let Some(slot) = self.window_slot(which) {
            slot.refresh_values();
        }
    }

    /// What the plugin last said one of its parameters was.
    pub(super) fn value(&self, which: PluginWindow, param: ParamId) -> Option<f32> {
        let slot = match which {
            PluginWindow::Effect(id) => self.slots.get(&id),
            PluginWindow::Instrument(id) => self.instruments.get(&id),
        };
        *slot?.values.get(param.index())?
    }

    /// Whether this plugin has a window of its own that could be opened.
    pub(super) fn has_window(&mut self, which: PluginWindow) -> bool {
        self.window_slot(which)
            .and_then(HostedSlot::plugin_mut)
            .is_some_and(ClapPlugin::has_gui)
    }

    /// Whether its window is meant to be on screen.
    pub(super) fn window_is_open(&self, which: PluginWindow) -> bool {
        let slot = match which {
            PluginWindow::Effect(id) => self.slots.get(&id),
            PluginWindow::Instrument(id) => self.instruments.get(&id),
        };
        slot.is_some_and(|slot| slot.editor)
    }

    /// Puts a plugin's window on screen or takes it off.
    pub(super) fn set_window_open(
        &mut self,
        which: PluginWindow,
        open: bool,
    ) -> Result<(), ClapError> {
        let parent = self.parent;
        let Some(slot) = self.window_slot(which) else {
            return Err(ClapError::NoGui {
                id: format!("{which:?}"),
                reason: "nothing hosted is here".into(),
            });
        };
        slot.editor = open;
        slot.settle_editor(parent)
    }

    /// Runs every hosted plugin's clocks and keeps its window where the slot says.
    ///
    /// Called from the session's tick. Returns whether any plugin reported that it had changed
    /// something about itself, which the caller owes the document a dirty mark for.
    pub(super) fn service(&mut self) -> bool {
        self.sweep_retired();
        let mut dirty = false;
        for slot in self.slots.values_mut().chain(self.instruments.values_mut()) {
            dirty |= slot.service();
        }
        self.settle_editors();
        dirty
    }

    /// Puts every window back on the instance that now answers for its slot.
    ///
    /// Also after a graph rebuild, and not only on the tick: a rebuild is the one thing that can
    /// move the answer, and waiting a frame to follow it would be a frame of the window and the
    /// parameter panel showing two different instances.
    fn settle_editors(&mut self) {
        let parent = self.parent;
        for slot in self.slots.values_mut().chain(self.instruments.values_mut()) {
            if let Err(error) = slot.settle_editor(parent) {
                log::warn!("`{}` would not show its window: {error}", slot.clap_id);
                // Or it would be asked again on the next tick, and every tick after that.
                slot.editor = false;
            }
        }
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

    /// Asks the plugin what its parameters are now.
    ///
    /// One pass over the whole list, which for Surge XT is seven hundred and seventy-five calls
    /// into somebody else's binary — so this is called when a panel is *drawn*, and not on the
    /// tick. A plugin nobody is looking at is a plugin whose values nobody can be misled by.
    fn refresh_values(&mut self) {
        let Some(plugin) = self.live.as_mut().or(self.spare.as_mut()) else {
            return;
        };
        let count = plugin.parameters().len();
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            values.push(plugin.value(ParamId(index as u32)));
        }
        self.values = values;
    }

    /// Runs the plugin's clocks and puts its window where the slot says it should be.
    ///
    /// Both instances get their timers run, not only the one being asked questions: a timer is a
    /// promise the host made when the plugin registered it, and an instance rendering in the graph
    /// is as entitled to it as one holding a window.
    fn service(&mut self) -> bool {
        let mut closed = false;
        let mut dirty = false;
        for plugin in [self.live.as_mut(), self.spare.as_mut()]
            .into_iter()
            .flatten()
        {
            plugin.tick_timers();
            let requests = plugin.take_requests();
            if requests.gui_closed {
                plugin.close_gui();
                closed = true;
            }
            // The plugin changed something about itself — a preset loaded in its own window, a
            // knob turned there. Nothing in the document has moved, and that is exactly the
            // problem: without this the title bar says saved, autosave writes nothing, and the
            // afternoon's work is inside a plugin that is about to be dropped.
            dirty |= requests.dirty;
            // `rescan_values` is deliberately not acted on. The values are read afresh every time
            // a panel is drawn — see `Session::effect_descriptors` — so being told they changed
            // asks for something that has already happened. A `restart` has no answer here yet,
            // and this is where it will go when it does.
        }
        // Somebody clicked the window's close box, which is the same thing as asking for it to be
        // shut. Reading it as anything else means the button that opens it does nothing next time
        // it is pressed, because as far as the slot is concerned it is already open.
        if closed {
            self.editor = false;
        }
        dirty
    }

    /// Gives the slot's one window to the instance the slot answers with, and to no other.
    fn settle_editor(&mut self, parent: Option<RawWindowHandle>) -> Result<(), ClapError> {
        let wanted = self.editor;
        // Ordered: the outgoing instance lets go before the incoming one asks, so a plugin that
        // will only have one window open at a time still ends up with one.
        if self.live.is_some()
            && let Some(spare) = self.spare.as_mut()
        {
            spare.close_gui();
        }
        let Some(plugin) = self.plugin_mut() else {
            return Ok(());
        };
        match wanted {
            true => plugin.open_gui(parent),
            false => {
                plugin.close_gui();
                Ok(())
            }
        }
    }

    /// Drains the slot's instances into `graveyard` on the way out.
    ///
    /// The way a slot leaves — never by plain drop. An instance whose effect is still in the
    /// engine's graph cannot be freed yet, and clack answers a drop at that moment by leaking
    /// it; parked instead, it is dropped by a later sweep once the effect has come home.
    fn retire_into(mut self, graveyard: &mut Vec<ClapPlugin>) {
        for mut plugin in self.live.take().into_iter().chain(self.spare.take()) {
            plugin.close_gui();
            graveyard.push(plugin);
        }
    }

    /// Produces the effect for the next graph, reusing an instance when one is free.
    fn take_effect(
        &mut self,
        library: &ClapLibrary,
        state: &PluginState,
        prepare: &PrepareContext,
        graveyard: &mut Vec<ClapPlugin>,
    ) -> Result<ClapEffect, ClapError> {
        let mut plugin = self.incoming(library, state, graveyard)?;
        let mut effect = plugin.activate(prepare)?;
        self.sidechain = effect.wants_sidechain();
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
        graveyard: &mut Vec<ClapPlugin>,
    ) -> Result<ClapInstrument, ClapError> {
        let mut plugin = self.incoming(library, state, graveyard)?;
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
        graveyard: &mut Vec<ClapPlugin>,
    ) -> Result<ClapPlugin, ClapError> {
        self.reclaim(graveyard);

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
    fn reclaim(&mut self, graveyard: &mut Vec<ClapPlugin>) {
        if self.live.as_mut().is_some_and(ClapPlugin::release) {
            self.spare = self.live.take();
        }
        // A spare with an effect still out is not a spare: handing it to `activate` would be told
        // the plugin is already active, and the slot would go silent for no reason a user could
        // see. It cannot simply be dropped either — clack would leak an instance whose effect is
        // still alive — so it is parked for a later sweep. Its state was copied forward when it
        // stopped being live.
        //
        // This is not the two-instance case; it is the three-instance one, which an export
        // running while the transport plays produces.
        if self.spare.as_mut().is_some_and(|spare| !spare.release())
            && let Some(spare) = self.spare.take()
        {
            graveyard.push(spare);
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
    graveyard: &mut Vec<ClapPlugin>,
) -> &'a mut HostedSlot {
    let fresh = || HostedSlot {
        file: file.to_path_buf(),
        clap_id: clap_id.to_string(),
        live: None,
        spare: None,
        // A different plugin's window is not this plugin's window, so a slot that was swapped out
        // from under an open editor comes back with none — and the one that was open closed with
        // the instance it belonged to.
        editor: false,
        sidechain: false,
        values: Vec::new(),
    };
    let slot = slots.entry(key).or_insert_with(fresh);
    if slot.clap_id != clap_id || slot.file != file {
        // The old plugin's instances retire rather than drop — the outgoing effect may still be
        // in the graph the engine holds, and dropping its instance now would leak it.
        std::mem::replace(slot, fresh()).retire_into(graveyard);
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
    /// Every `.clap` file installed in the usual places, plus anywhere else asked for.
    ///
    /// Only the files: opening them is what [`Self::hosted_plugins_in`] does, and a browser that
    /// loaded every plugin on the machine to draw a list would run their code to do it.
    ///
    /// `extra` is where somebody keeps plugins that the conventional folders do not cover — a
    /// build tree, a bounced copy on an external disk, a shared folder on a studio machine.
    /// Each entry is either a `.clap` taken as it stands or a directory walked like the standard
    /// roots, so pointing at one plugin and pointing at a hundred are the same gesture.
    pub fn installed_clap_files(&self, extra: &[PathBuf]) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for root in clap_search_paths() {
            collect_clap_files(&root, &mut found);
        }
        for root in extra {
            // A `.clap` bundle on macOS is a directory, so walking into it would find nothing
            // and drop the plugin somebody explicitly pointed at. The extension decides first,
            // exactly as it does inside the walk.
            match root
                .extension()
                .is_some_and(|extension| extension == "clap")
            {
                true => found.push(root.clone()),
                false => collect_clap_files(root, &mut found),
            }
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

    /// Names the window a plugin's own window should float above.
    ///
    /// A frontend calls this once, with its main window. Passing `None` — or never calling it at
    /// all — still gives working plugin windows; they behave like any other top-level window,
    /// which on Windows means sinking behind the application the moment it is clicked.
    ///
    /// The handle is kept rather than passed in each time, because a window that has to be moved
    /// between instances needs it at a moment nobody asked for anything. Nothing here can check
    /// that it still names a window: what makes it safe is that it names the application's own,
    /// and dropping the session closes every plugin window before that can go.
    pub fn set_plugin_window_parent(&mut self, parent: Option<RawWindowHandle>) {
        self.hosted.parent = parent;
    }

    /// Whether this plugin has a window of its own that could be opened.
    ///
    /// `false` for everything that is not a hosted plugin, for a hosted plugin that draws nothing,
    /// and for one whose windowing this platform cannot provide. A frontend should not offer the
    /// command when this is `false` — there is nothing behind it.
    pub fn plugin_window_exists(&mut self, which: PluginWindow) -> bool {
        self.hosted.has_window(which)
    }

    /// Whether a plugin's own window is on screen.
    pub fn plugin_window_is_open(&self, which: PluginWindow) -> bool {
        self.hosted.window_is_open(which)
    }

    /// Opens or closes a plugin's own window.
    ///
    /// Not an undoable edit and not part of the document: which windows are open is a fact about
    /// this sitting, like which panel is showing. A project that reopened four plugin windows
    /// because it was saved with four open would be a project that took over the screen.
    pub fn set_plugin_window_open(
        &mut self,
        which: PluginWindow,
        open: bool,
    ) -> Result<(), SessionError> {
        Ok(self.hosted.set_window_open(which, open)?)
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
        // A panel asking what to draw is the moment somebody is about to look at these, and the
        // only moment worth reading a hosted plugin's own values at — see
        // [`Self::param_value`](Session::param_value) for why they cannot simply come from the
        // document.
        self.hosted.refresh_values(PluginWindow::Instrument(track));
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
        self.hosted.refresh_values(PluginWindow::Effect(slot));
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
            editor: false,
            sidechain: false,
            values: Vec::new(),
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
            .take_effect(&library, &state, &prepare(), &mut Vec::new())
            .expect("first");
        assert!(slot.live.is_some());
        assert!(slot.spare.is_none(), "nothing has been freed yet");

        let second = slot
            .take_effect(&library, &state, &prepare(), &mut Vec::new())
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
            .take_effect(&library, &state, &prepare(), &mut Vec::new())
            .expect("first");
        drop(first); // the engine returned the graph and it was dropped

        let second = slot
            .take_effect(&library, &state, &prepare(), &mut Vec::new())
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
            .take_effect(&library, &state, &prepare(), &mut Vec::new())
            .expect("first");
        first.set_param(ParamId(0), 0.5);
        assert_eq!(render(&mut first, 1.0), 0.5);
        drop(first);

        let mut second = slot
            .take_effect(&library, &state, &prepare(), &mut Vec::new())
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
            .take_effect(&library, &patched, &prepare(), &mut Vec::new())
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

    #[test]
    fn an_instance_let_go_mid_flight_is_parked_until_its_effect_comes_home() {
        // Dropping a `ClapPlugin` whose effect is still in the engine's graph does not free it:
        // clack answers that drop by leaking the instance. So a slot leaving while its effect is
        // still out has to park its instances, and only a sweep after the effect has come home
        // may drop them for real.
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

        let first = hosted.place(&project, &prepare());
        assert_eq!(first.len(), 1);

        // The slot leaves the project while `first` — the engine's graph — is still alive.
        project.remove_effect(slot);
        let second = hosted.place(&project, &prepare());
        assert!(second.is_empty());
        assert_eq!(
            hosted.retiring.len(),
            1,
            "the live instance is parked, not dropped"
        );
        hosted.sweep_retired();
        assert_eq!(
            hosted.retiring.len(),
            1,
            "and stays parked while its effect is still out"
        );

        // The engine hands the old graph back and it is dropped off the audio thread.
        drop(first);
        hosted.sweep_retired();
        assert!(
            hosted.retiring.is_empty(),
            "freed once its effect came home"
        );
    }

    #[test]
    fn an_unrelated_undo_keeps_the_hosted_slot_alive() {
        // Undo restores a variant of the same document, whose slot ids still name the same
        // plugins. Clearing the slots on every document swap — as undo once did — dropped a
        // live instance whose effect was still in the engine's graph (a leak, see above) and
        // threw away plugin-internal state the undo was never aimed at.
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .add_hosted_effect(Some(track), &file, FIXTURE_ID)
            .expect("the fixture is in the library");
        assert_eq!(session.hosted.slots.len(), 1);

        session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session.undo();
        assert_eq!(
            session.hosted.slots.len(),
            1,
            "an unrelated undo must not evict the slot"
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
        assert_eq!(session.hosted_instrument_parameters(track).len(), 4);
        assert_eq!(session.hosted_instrument_name(track), Some("Test Tone"));
        assert_eq!(
            session.instrument_descriptors(track).len(),
            4,
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
            .render(
                &auris_engine::OfflineOptions::whole_project(),
                &mut auris_engine::RenderProgress::default(),
            )
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
    fn a_hosted_plugin_that_declares_a_sidechain_port_is_offered_a_source() {
        // The fixture declares one; the built-in reverb does not. A frontend draws the picker off
        // this answer, and it has to come from the *plugin* — the registry has never heard of a
        // hosted one, and two slots can hold the same plugin out of two different files.
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let hosted = session
            .add_hosted_effect(Some(track), &file, FIXTURE_ID)
            .expect("the fixture is in the library");
        let built_in = session.add_effect(Some(track), "auris.fx.reverb").unwrap();

        assert!(session.effect_wants_sidechain(Some(track), hosted));
        assert!(!session.effect_wants_sidechain(Some(track), built_in));
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
            5,
            "the fixture's gain and its four reports back about what the host did"
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
        assert_eq!(session.hosted_parameters(slot).len(), 5);
    }

    #[test]
    fn a_plugins_own_window_is_opened_and_closed_by_the_slot_it_sits_in() {
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let slot = session
            .add_hosted_effect(Some(track), &file, FIXTURE_ID)
            .unwrap();
        let which = PluginWindow::Effect(slot);

        assert!(session.plugin_window_exists(which));
        assert!(!session.plugin_window_is_open(which));

        session.set_plugin_window_open(which, true).expect("opens");
        assert!(session.plugin_window_is_open(which));

        session
            .set_plugin_window_open(which, false)
            .expect("closes");
        assert!(!session.plugin_window_is_open(which));
    }

    #[test]
    fn a_track_with_no_hosted_plugin_offers_no_window() {
        // What the button is drawn from. A built-in instrument has parameters and no window, and
        // asking for one has to be an answer rather than a panic.
        let mut session = super::super::fixtures::session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let which = PluginWindow::Instrument(track);

        assert!(!session.plugin_window_exists(which));
        assert!(!session.plugin_window_is_open(which));
        assert!(session.set_plugin_window_open(which, true).is_err());
    }

    #[test]
    fn a_window_the_user_closed_stops_the_slot_claiming_it_is_open() {
        // Otherwise the button that opens it does nothing the next time it is pressed: the slot
        // still thinks the window is up, so it asks for nothing, and the plugin — which threw its
        // window away — draws nothing.
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let slot = session
            .add_hosted_effect(Some(track), &file, FIXTURE_ID)
            .unwrap();
        let which = PluginWindow::Effect(slot);
        session.set_plugin_window_open(which, true).unwrap();

        session
            .hosted
            .window_slot(which)
            .and_then(|slot| slot.plugin_mut())
            .expect("a built instance")
            .pretend_the_window_closed(true);

        session.poll();
        assert!(!session.plugin_window_is_open(which));
    }

    #[test]
    fn an_open_window_follows_the_instance_that_answers_for_the_slot() {
        // The one thing the two-instance handover could get wrong that nothing else would notice:
        // a window drawn by one instance beside a parameter panel reading the other. The rebuild
        // below is the rapid kind — the first effect is still out — which is exactly the case that
        // makes a second instance.
        let library = fixture_library();
        let mut slot = slot();
        let state = PluginState::empty();

        let first = slot
            .take_effect(&library, &state, &prepare(), &mut Vec::new())
            .expect("first");
        slot.editor = true;
        slot.settle_editor(None).expect("opens");
        assert!(slot.live.as_ref().unwrap().gui_is_open());

        let second = slot
            .take_effect(&library, &state, &prepare(), &mut Vec::new())
            .expect("second");
        assert!(slot.spare.is_some(), "the handover really did make a pair");
        slot.settle_editor(None).expect("moves");

        assert!(
            slot.live.as_ref().unwrap().gui_is_open(),
            "the window belongs to whichever instance the slot now answers with"
        );
        assert!(
            !slot.spare.as_ref().unwrap().gui_is_open(),
            "and to no other, or the plugin has two windows open onto one slot"
        );

        drop(first);
        drop(second);
    }

    #[test]
    fn a_hosted_parameter_reads_back_what_the_plugin_says_rather_than_a_default() {
        // What the plugin's own window made visible: Surge XT showed Delay with a feedback of
        // 50 %, and the panel beside it showed thirteen zeros. The panel only ever knew what Auris
        // itself had written into the document, and Auris had written nothing — so every control
        // fell back to a default the plugin had long since moved away from.
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        // Placed on the project rather than through the command, so the patch below is in place
        // before the plugin is built for the first time — which is how a saved project arrives.
        let slot = session
            .project
            .add_hosted_effect(
                Some(track),
                "clap:studio.auris.test.gain",
                AssetPath::external(&file),
            )
            .unwrap();
        session.project.tracks[0].mixer.effects[0]
            .state
            .set_hosted_bytes(&0.25f32.to_le_bytes());
        session.rebuild_graph();

        let target = crate::param::ParamTarget::Effect {
            track: Some(track),
            slot,
            param: ParamId(0),
        };
        // Drawing the panel is what reads the plugin, so this is the call a frontend makes first.
        let descriptors = session.effect_descriptors(slot);
        let descriptor = descriptors[0].clone();
        assert_eq!(
            descriptor.default, 1.0,
            "or a default could be mistaken for an answer"
        );
        assert_eq!(
            session.param_value(target, &descriptor),
            0.25,
            "the patch's value, which is in the plugin and nowhere else"
        );

        // And what the user set in Auris still beats it, which is the same order state is
        // restored in — a drag that lost to the patch would spring back under the pointer.
        session.set_param(target, 2.0);
        assert_eq!(session.param_value(target, &descriptor), 2.0);
    }

    #[test]
    fn a_preset_loaded_inside_a_plugin_puts_the_unsaved_mark_back() {
        // A plugin's own window is a way of editing the project that the session never sees. The
        // plugin says so by setting one flag, and until that flag was read the title bar said
        // saved, autosave wrote nothing, and an afternoon inside Surge XT could be closed away.
        let (mut session, file) = session_with_fixture();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let slot = session
            .add_hosted_effect(Some(track), &file, FIXTURE_ID)
            .unwrap();

        session.dirty = false;
        session
            .hosted
            .window_slot(PluginWindow::Effect(slot))
            .and_then(|slot| slot.plugin_mut())
            .expect("a built instance")
            .pretend_the_state_changed();
        assert!(!session.is_dirty(), "nothing has looked yet");

        session.poll();
        assert!(session.is_dirty());
    }

    #[test]
    fn a_built_in_plugins_value_still_comes_from_the_document_alone() {
        // The other half: nothing but the document can move a built-in's parameter, and a
        // fallback that started answering for one would be answering from an empty cache.
        let mut session = super::super::fixtures::session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let descriptors = session.instrument_descriptors(track);
        let descriptor = descriptors
            .first()
            .expect("a built-in has parameters")
            .clone();
        let target = crate::param::ParamTarget::Instrument {
            track,
            param: descriptor.id,
        };

        assert_eq!(session.param_value(target, &descriptor), descriptor.default);
        session.set_param(target, descriptor.max);
        assert_eq!(session.param_value(target, &descriptor), descriptor.max);
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
