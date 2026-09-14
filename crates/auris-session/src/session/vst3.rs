//! VST3 instances owned by an editing session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use auris_core::asset::AssetPath;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{PluginState, PrepareContext};
use auris_core::project::{EffectSlotId, Project, TrackId};
use auris_engine::{PlacedEffects, PlacedInstruments};
use auris_vst3::{Vst3Error, Vst3Plugin, Vst3PluginInfo};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

#[derive(Default)]
pub(super) struct Vst3Plugins {
    slots: BTreeMap<EffectSlotId, Vst3Slot>,
    instruments: BTreeMap<TrackId, Vst3Slot>,
    retiring: Vec<Vst3Plugin>,
    render_error: Option<Vst3Error>,
}

struct Vst3Slot {
    file: PathBuf,
    class_id: String,
    plugin: Vst3Plugin,
    needs_state_restore: bool,
    source_revision: u64,
}

struct Request<'a> {
    file: PathBuf,
    class_id: String,
    state: &'a PluginState,
}

impl Vst3Plugins {
    /// Commits a source that was loaded before the composed document replaced its predecessor.
    pub(super) fn install_composed_instrument(
        &mut self,
        track: TrackId,
        file: PathBuf,
        class_id: String,
        plugin: Vst3Plugin,
    ) {
        if let Some(old) = self.instruments.remove(&track) {
            self.retiring.push(old.plugin);
        }
        self.instruments.insert(
            track,
            Vst3Slot {
                file,
                class_id,
                plugin,
                needs_state_restore: true,
                source_revision: 0,
            },
        );
    }

    pub(super) fn drum_source_revision(&self, track: TrackId) -> Option<u64> {
        Some(self.instruments.get(&track)?.source_revision)
    }

    /// Consumes native editor notifications and mirrors their values into the document.
    ///
    /// Parameter values already reached renderers through their atomic snapshots. Opaque preset
    /// and lifecycle changes request a fresh renderer because they cannot be represented by one
    /// normalized value.
    pub(super) fn service(&mut self, project: &mut Project) -> (bool, bool) {
        match self.service_inner(project, false) {
            Ok(result) => result,
            Err(error) => {
                self.remember_render_error(error);
                (false, false)
            }
        }
    }

    pub(super) fn service_for_save(
        &mut self,
        project: &mut Project,
    ) -> Result<(bool, bool), Vst3Error> {
        match self.service_inner(project, true) {
            Ok(result) => Ok(result),
            Err(Vst3Error::StateSync {
                plugin,
                operation,
                detail,
            }) => {
                self.remember_render_error(Vst3Error::StateSync {
                    plugin: plugin.clone(),
                    operation,
                    detail: detail.clone(),
                });
                Err(Vst3Error::StateSync {
                    plugin,
                    operation,
                    detail,
                })
            }
            Err(error) => {
                let detail = error.to_string();
                self.remember_render_error(Vst3Error::StateSync {
                    plugin: "VST3 plugin".into(),
                    operation: "captured for saving",
                    detail: detail.clone(),
                });
                Err(Vst3Error::StateSync {
                    plugin: "VST3 plugin".into(),
                    operation: "captured for saving",
                    detail,
                })
            }
        }
    }

    fn service_inner(
        &mut self,
        project: &mut Project,
        wait_for_editor: bool,
    ) -> Result<(bool, bool), Vst3Error> {
        let mut dirty = false;
        let mut rebuild = false;
        let mut poll_error = None;
        for (id, slot) in &mut self.slots {
            let captured = if wait_for_editor {
                let (changes, state) = slot.plugin.take_source_changes_for_save()?;
                Ok((changes, Some(state)))
            } else {
                slot.plugin
                    .take_source_changes()
                    .map(|changes| (changes, None))
            };
            let (changes, captured_state) = match captured {
                Ok(captured) => captured,
                Err(error) => {
                    // A polling failure for one plugin must not discard the dirty/rebuild flags
                    // already produced by another plugin in this pass. The failed source keeps a
                    // full-refresh request and will retry on the next poll.
                    if poll_error.is_none() {
                        poll_error = Some(error);
                    }
                    continue;
                }
            };
            if let Some(failure) = changes.renderer_failure
                && self.render_error.is_none()
            {
                let error = Vst3Error::RendererStopped {
                    plugin: slot.plugin.info().name.clone(),
                    failure,
                };
                log::error!("{error}");
                self.render_error = Some(error);
            }
            if changes.source_changed {
                slot.source_revision = slot.source_revision.wrapping_add(1);
                dirty = true;
            }
            rebuild |= changes.renderer_reload_required;
            for (param, value) in changes.values {
                let Some(descriptor) = slot.plugin.parameters().get(param.index()) else {
                    continue;
                };
                for strip in std::iter::once(&mut project.master)
                    .chain(project.tracks.iter_mut().map(|track| &mut track.mixer))
                {
                    if let Some(effect) = strip.effects.iter_mut().find(|effect| effect.id == *id) {
                        effect
                            .state
                            .params
                            .insert(descriptor.key.to_string(), value);
                        break;
                    }
                }
            }
            if let Some(bytes) = captured_state {
                for strip in std::iter::once(&mut project.master)
                    .chain(project.tracks.iter_mut().map(|track| &mut track.mixer))
                {
                    if let Some(effect) = strip.effects.iter_mut().find(|effect| effect.id == *id) {
                        effect.state.set_hosted_bytes(&bytes);
                        break;
                    }
                }
            }
        }
        for (track, slot) in &mut self.instruments {
            let captured = if wait_for_editor {
                let (changes, state) = slot.plugin.take_source_changes_for_save()?;
                Ok((changes, Some(state)))
            } else {
                slot.plugin
                    .take_source_changes()
                    .map(|changes| (changes, None))
            };
            let (changes, captured_state) = match captured {
                Ok(captured) => captured,
                Err(error) => {
                    if poll_error.is_none() {
                        poll_error = Some(error);
                    }
                    continue;
                }
            };
            if let Some(failure) = changes.renderer_failure
                && self.render_error.is_none()
            {
                let error = Vst3Error::RendererStopped {
                    plugin: slot.plugin.info().name.clone(),
                    failure,
                };
                log::error!("{error}");
                self.render_error = Some(error);
            }
            if changes.source_changed {
                slot.source_revision = slot.source_revision.wrapping_add(1);
                dirty = true;
            }
            rebuild |= changes.renderer_reload_required;
            let Some(state) = project
                .track_mut(*track)
                .and_then(|track| track.kind.as_instrument_mut())
                .map(|instrument| &mut instrument.instrument_state)
            else {
                continue;
            };
            if let Some(bytes) = captured_state {
                state.set_hosted_bytes(&bytes);
            }
            for (param, value) in changes.values {
                let Some(descriptor) = slot.plugin.parameters().get(param.index()) else {
                    continue;
                };
                state.params.insert(descriptor.key.to_string(), value);
            }
        }
        self.sweep();
        if let Some(error) = poll_error {
            self.remember_render_error(error);
        }
        Ok((dirty, rebuild))
    }

    pub(super) fn take_render_error(&mut self) -> Option<Vst3Error> {
        self.render_error.take()
    }

    fn remember_render_error(&mut self, error: Vst3Error) {
        log::error!("{error}");
        if self.render_error.is_none() {
            self.render_error = Some(error);
        }
    }

    pub(super) fn set_effect_param(
        &mut self,
        slot: EffectSlotId,
        param: ParamId,
        value: f32,
    ) -> Result<bool, auris_vst3::Vst3Error> {
        let Some(slot) = self.slots.get(&slot) else {
            return Ok(false);
        };
        let name = slot.plugin.info().name.clone();
        match slot.plugin.set_param(param, value) {
            Ok(()) => Ok(true),
            Err(source) => Err(self.remember_state_error(name, "updated", source)),
        }
    }

    pub(super) fn set_instrument_param(
        &mut self,
        track: TrackId,
        param: ParamId,
        value: f32,
    ) -> Result<bool, auris_vst3::Vst3Error> {
        let Some(slot) = self.instruments.get(&track) else {
            return Ok(false);
        };
        let name = slot.plugin.info().name.clone();
        match slot.plugin.set_param(param, value) {
            Ok(()) => Ok(true),
            Err(source) => Err(self.remember_state_error(name, "updated", source)),
        }
    }

    /// Snapshots the actual instrument for an independent measurement worker.
    pub(super) fn drum_probe_state(
        &self,
        track: TrackId,
        saved: &PluginState,
    ) -> Result<PluginState, SessionError> {
        let plugin = &self
            .instruments
            .get(&track)
            .ok_or_else(|| SessionError::DrumAnalysis("the VST3 instrument is unavailable".into()))?
            .plugin;
        let mut state = saved.clone();
        state.set_hosted_bytes(&plugin.save_state()?);
        for descriptor in plugin.parameters() {
            let value = plugin.value(descriptor.id).ok_or_else(|| {
                SessionError::DrumAnalysis(
                    "the VST3 instrument could not snapshot a parameter".into(),
                )
            })?;
            state.params.insert(descriptor.key.to_string(), value);
        }
        Ok(state)
    }

    pub(super) fn clear(&mut self) {
        self.render_error = None;
        self.retiring.extend(
            std::mem::take(&mut self.slots)
                .into_values()
                .map(|slot| slot.plugin),
        );
        self.retiring.extend(
            std::mem::take(&mut self.instruments)
                .into_values()
                .map(|slot| slot.plugin),
        );
        self.sweep();
    }

    pub(super) fn sweep(&mut self) {
        self.retiring.retain(|plugin| !plugin.is_idle());
    }

    pub(super) fn place(&mut self, project: &Project, prepare: &PrepareContext) -> PlacedEffects {
        self.sweep();
        let wanted = effect_requests(project);
        retire_missing(&mut self.slots, &wanted, &mut self.retiring);
        let mut placed = PlacedEffects::new();
        for (id, request) in wanted {
            let rendered = {
                let slot = match fit(&mut self.slots, id, &request, prepare, &mut self.retiring) {
                    Ok(slot) => slot,
                    Err(error) => {
                        self.remember_render_error(error);
                        continue;
                    }
                };
                restore(slot, request.state).and_then(|()| slot.plugin.effect())
            };
            match rendered {
                Ok(effect) => {
                    placed.insert(id, Box::new(effect));
                }
                Err(error) => self.remember_render_error(error),
            }
        }
        placed
    }

    pub(super) fn place_instruments(
        &mut self,
        project: &Project,
        prepare: &PrepareContext,
    ) -> PlacedInstruments {
        self.sweep();
        let wanted = instrument_requests(project);
        retire_missing(&mut self.instruments, &wanted, &mut self.retiring);
        let mut placed = PlacedInstruments::new();
        for (track, request) in wanted {
            let slot = match fit(
                &mut self.instruments,
                track,
                &request,
                prepare,
                &mut self.retiring,
            ) {
                Ok(slot) => slot,
                Err(error) => {
                    self.remember_render_error(error);
                    continue;
                }
            };
            let rendered = restore(slot, request.state).and_then(|()| slot.plugin.instrument());
            match rendered {
                Ok(instrument) => {
                    placed.insert(track, Box::new(instrument));
                }
                Err(error) => self.remember_render_error(error),
            }
        }
        placed
    }

    pub(super) fn parameters(&self, slot: EffectSlotId) -> Option<&[ParamDescriptor]> {
        Some(self.slots.get(&slot)?.plugin.parameters())
    }

    pub(super) fn instrument_parameters(&self, track: TrackId) -> Option<&[ParamDescriptor]> {
        Some(self.instruments.get(&track)?.plugin.parameters())
    }

    pub(super) fn name(&self, slot: EffectSlotId) -> Option<&str> {
        Some(&self.slots.get(&slot)?.plugin.info().name)
    }

    pub(super) fn instrument_name(&self, track: TrackId) -> Option<&str> {
        Some(&self.instruments.get(&track)?.plugin.info().name)
    }

    pub(super) fn wants_sidechain(&self, slot: EffectSlotId) -> bool {
        self.slots
            .get(&slot)
            .is_some_and(|slot| slot.plugin.wants_sidechain())
    }

    pub(super) fn value_effect(&self, slot: EffectSlotId, id: ParamId) -> Option<f32> {
        self.slots.get(&slot)?.plugin.value(id)
    }

    pub(super) fn value_instrument(&self, track: TrackId, id: ParamId) -> Option<f32> {
        self.instruments.get(&track)?.plugin.value(id)
    }

    pub(super) fn save_effect(&mut self, slot: EffectSlotId) -> Result<Option<Vec<u8>>, Vst3Error> {
        let Some(slot) = self.slots.get(&slot) else {
            return Ok(None);
        };
        let name = slot.plugin.info().name.clone();
        match slot.plugin.save_state() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(source) => Err(self.remember_state_error(name, "saved", source)),
        }
    }

    pub(super) fn save_instrument(&mut self, track: TrackId) -> Result<Option<Vec<u8>>, Vst3Error> {
        let Some(slot) = self.instruments.get(&track) else {
            return Ok(None);
        };
        let name = slot.plugin.info().name.clone();
        match slot.plugin.save_state() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(source) => Err(self.remember_state_error(name, "saved", source)),
        }
    }

    fn remember_state_error(
        &mut self,
        plugin: String,
        operation: &'static str,
        source: Vst3Error,
    ) -> Vst3Error {
        let detail = source.to_string();
        self.remember_render_error(Vst3Error::StateSync {
            plugin: plugin.clone(),
            operation,
            detail: detail.clone(),
        });
        Vst3Error::StateSync {
            plugin,
            operation,
            detail,
        }
    }

    pub(super) fn has_effect_window(&self, slot: EffectSlotId) -> bool {
        self.slots
            .get(&slot)
            .is_some_and(|slot| slot.plugin.has_gui())
    }

    pub(super) fn has_instrument_window(&self, track: TrackId) -> bool {
        self.instruments
            .get(&track)
            .is_some_and(|slot| slot.plugin.has_gui())
    }

    pub(super) fn effect_window_is_open(&self, slot: EffectSlotId) -> bool {
        self.slots
            .get(&slot)
            .is_some_and(|slot| slot.plugin.gui_is_open())
    }

    pub(super) fn instrument_window_is_open(&self, track: TrackId) -> bool {
        self.instruments
            .get(&track)
            .is_some_and(|slot| slot.plugin.gui_is_open())
    }

    pub(super) fn set_effect_window_open(
        &mut self,
        slot: EffectSlotId,
        open: bool,
    ) -> Result<bool, auris_vst3::Vst3Error> {
        let Some(slot) = self.slots.get_mut(&slot) else {
            return Ok(false);
        };
        slot.plugin.set_gui_open(open)?;
        Ok(true)
    }

    pub(super) fn set_instrument_window_open(
        &mut self,
        track: TrackId,
        open: bool,
    ) -> Result<bool, auris_vst3::Vst3Error> {
        let Some(slot) = self.instruments.get_mut(&track) else {
            return Ok(false);
        };
        slot.plugin.set_gui_open(open)?;
        Ok(true)
    }
}

fn fit<'a, K: Ord + Copy>(
    slots: &'a mut BTreeMap<K, Vst3Slot>,
    key: K,
    request: &Request<'_>,
    prepare: &PrepareContext,
    retiring: &mut Vec<Vst3Plugin>,
) -> Result<&'a mut Vst3Slot, Vst3Error> {
    let replace = slots
        .get(&key)
        .is_some_and(|slot| slot.file != request.file || slot.class_id != request.class_id);
    if replace && let Some(old) = slots.remove(&key) {
        retiring.push(old.plugin);
    }
    if let std::collections::btree_map::Entry::Vacant(entry) = slots.entry(key) {
        match Vst3Plugin::load(&request.file, &request.class_id, prepare) {
            Ok(plugin) => {
                entry.insert(Vst3Slot {
                    file: request.file.clone(),
                    class_id: request.class_id.clone(),
                    plugin,
                    needs_state_restore: true,
                    source_revision: 0,
                });
            }
            Err(error) => {
                return Err(error);
            }
        }
    }
    slots
        .get_mut(&key)
        .ok_or_else(|| Vst3Error::UnknownPlugin(request.class_id.clone()))
}

fn restore(slot: &mut Vst3Slot, state: &PluginState) -> Result<(), Vst3Error> {
    if slot.needs_state_restore {
        if let Some(bytes) = state.hosted_bytes() {
            slot.plugin.load_state(&bytes)?;
        }
        slot.needs_state_restore = false;
    }
    for descriptor in slot.plugin.parameters() {
        if let Some(value) = state.params.get(descriptor.key.as_ref()) {
            slot.plugin.set_param(descriptor.id, *value)?;
        }
    }
    Ok(())
}

fn retire_missing<K: Ord + Copy>(
    slots: &mut BTreeMap<K, Vst3Slot>,
    wanted: &BTreeMap<K, Request<'_>>,
    retiring: &mut Vec<Vst3Plugin>,
) {
    let leaving: Vec<K> = slots
        .keys()
        .filter(|key| !wanted.contains_key(key))
        .copied()
        .collect();
    for key in leaving {
        if let Some(slot) = slots.remove(&key) {
            retiring.push(slot.plugin);
        }
    }
}

fn effect_requests(project: &Project) -> BTreeMap<EffectSlotId, Request<'_>> {
    std::iter::once(&project.master)
        .chain(project.tracks.iter().map(|track| &track.mixer))
        .flat_map(|strip| &strip.effects)
        .filter(|slot| slot.effect_id.starts_with(auris_vst3::ID_PREFIX))
        .filter_map(|slot| {
            Some((
                slot.id,
                Request {
                    file: slot.file.as_ref()?.resolve_for_automatic_access(None)?,
                    class_id: slot
                        .effect_id
                        .strip_prefix(auris_vst3::ID_PREFIX)?
                        .to_string(),
                    state: &slot.state,
                },
            ))
        })
        .collect()
}

fn instrument_requests(project: &Project) -> BTreeMap<TrackId, Request<'_>> {
    project
        .tracks
        .iter()
        .filter_map(|track| {
            let instrument = track.kind.as_instrument()?;
            let class_id = instrument
                .instrument_id
                .strip_prefix(auris_vst3::ID_PREFIX)?;
            Some((
                track.id,
                Request {
                    file: instrument
                        .file
                        .as_ref()?
                        .resolve_for_automatic_access(None)?,
                    class_id: class_id.to_string(),
                    state: &instrument.instrument_state,
                },
            ))
        })
        .collect()
}

impl Session {
    /// Takes a pending failure from an independently owned live VST3 renderer.
    ///
    /// The renderer stays stopped so it cannot replay a partial MIDI/automation batch. This is
    /// a one-shot notification for frontends; changing the graph creates a fresh instance.
    pub fn take_vst3_render_error(&mut self) -> Option<auris_vst3::Vst3Error> {
        self.vst3.take_render_error()
    }

    /// Every installed `.vst3` bundle in standard and user-selected plugin paths.
    pub fn installed_vst3_files(&self, extra: &[PathBuf]) -> Vec<PathBuf> {
        auris_vst3::installed_vst3_files(extra)
    }

    /// Inspects one VST3 bundle and lists its audio classes.
    pub fn vst3_plugins_in(&mut self, file: &Path) -> Result<Vec<Vst3PluginInfo>, SessionError> {
        Ok(auris_vst3::plugins_in(file)?)
    }

    /// Adds a VST3 effect to a track chain or the master bus.
    pub fn add_vst3_effect(
        &mut self,
        track: Option<TrackId>,
        file: &Path,
        class_id: &str,
    ) -> Result<EffectSlotId, SessionError> {
        if let Some(track) = track {
            self.require_track(track)?;
        }
        let known = self
            .vst3_plugins_in(file)?
            .into_iter()
            .find(|info| info.class_id == class_id)
            .ok_or_else(|| SessionError::UnknownPlugin(class_id.to_string()))?;
        self.record(Edit::AddEffect);
        let slot = self
            .project
            .add_hosted_effect(track, known.auris_id(), AssetPath::external(file))
            .ok_or_else(|| SessionError::UnknownTrack(track.map_or(0, |track| track.0)))?;
        self.invalidate_graph();
        Ok(slot)
    }

    /// Replaces a track's instrument with a VST3 instrument class.
    pub fn set_vst3_instrument(
        &mut self,
        track: TrackId,
        file: &Path,
        class_id: &str,
    ) -> Result<(), SessionError> {
        self.require_track(track)?;
        let known = self
            .vst3_plugins_in(file)?
            .into_iter()
            .find(|info| info.class_id == class_id)
            .ok_or_else(|| SessionError::UnknownPlugin(class_id.to_string()))?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::AssetPath;

    #[test]
    fn a_network_plugin_named_by_a_document_is_not_loaded_automatically() {
        let mut project = Project::new("VST3", 48_000.0);
        let slot = project
            .add_hosted_effect(
                None,
                format!("{}class", auris_vst3::ID_PREFIX),
                AssetPath::external(r"\\server\share\test.vst3"),
            )
            .unwrap();

        assert!(!effect_requests(&project).contains_key(&slot));
    }
}
