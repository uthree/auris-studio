//! VST3 instances owned by an editing session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use auris_core::asset::AssetPath;
use auris_core::param::{ParamDescriptor, ParamId};
use auris_core::plugin::{PluginState, PrepareContext};
use auris_core::project::{EffectSlotId, Project, TrackId};
use auris_engine::{PlacedEffects, PlacedInstruments};
use auris_vst3::{Vst3Plugin, Vst3PluginInfo};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

#[derive(Default)]
pub(super) struct Vst3Plugins {
    slots: BTreeMap<EffectSlotId, Vst3Slot>,
    instruments: BTreeMap<TrackId, Vst3Slot>,
    retiring: Vec<Vst3Plugin>,
}

struct Vst3Slot {
    file: PathBuf,
    class_id: String,
    plugin: Vst3Plugin,
    needs_state_restore: bool,
}

struct Request<'a> {
    file: PathBuf,
    class_id: String,
    state: &'a PluginState,
}

impl Vst3Plugins {
    pub(super) fn clear(&mut self) {
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
                let Some(slot) = fit(&mut self.slots, id, &request, prepare, &mut self.retiring)
                else {
                    continue;
                };
                restore(slot, request.state);
                effect_for_render(slot, prepare)
            };
            if let Some((effect, temporary)) = rendered {
                placed.insert(id, Box::new(effect));
                if let Some(plugin) = temporary {
                    self.retiring.push(plugin);
                }
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
            let Some(slot) = fit(
                &mut self.instruments,
                track,
                &request,
                prepare,
                &mut self.retiring,
            ) else {
                continue;
            };
            restore(slot, request.state);
            let rendered = instrument_for_render(slot, prepare);
            if let Some((instrument, temporary)) = rendered {
                placed.insert(track, Box::new(instrument));
                if let Some(plugin) = temporary {
                    self.retiring.push(plugin);
                }
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

    pub(super) fn save_effect(&self, slot: EffectSlotId) -> Option<Vec<u8>> {
        self.slots.get(&slot)?.plugin.save_state().ok()
    }

    pub(super) fn save_instrument(&self, track: TrackId) -> Option<Vec<u8>> {
        self.instruments.get(&track)?.plugin.save_state().ok()
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

fn effect_for_render(
    slot: &Vst3Slot,
    prepare: &PrepareContext,
) -> Option<(auris_vst3::Vst3Effect, Option<Vst3Plugin>)> {
    if slot.plugin.is_idle() {
        return slot.plugin.effect().ok().map(|effect| (effect, None));
    }
    let temporary = independent_instance(slot, prepare)?;
    let effect = temporary.effect().ok()?;
    Some((effect, Some(temporary)))
}

fn instrument_for_render(
    slot: &Vst3Slot,
    prepare: &PrepareContext,
) -> Option<(auris_vst3::Vst3Instrument, Option<Vst3Plugin>)> {
    if slot.plugin.is_idle() {
        return slot
            .plugin
            .instrument()
            .ok()
            .map(|instrument| (instrument, None));
    }
    let temporary = independent_instance(slot, prepare)?;
    let instrument = temporary.instrument().ok()?;
    Some((instrument, Some(temporary)))
}

fn independent_instance(slot: &Vst3Slot, prepare: &PrepareContext) -> Option<Vst3Plugin> {
    let plugin = match Vst3Plugin::load(&slot.file, &slot.class_id, prepare) {
        Ok(plugin) => plugin,
        Err(error) => {
            log::warn!(
                "cannot create an independent VST3 `{}` instance: {error}",
                slot.class_id
            );
            return None;
        }
    };
    if let Ok(state) = slot.plugin.save_state()
        && let Err(error) = plugin.load_state(&state)
    {
        log::warn!("cannot copy VST3 `{}` state: {error}", slot.class_id);
    }
    for descriptor in slot.plugin.parameters() {
        if let Some(value) = slot.plugin.value(descriptor.id) {
            let _ = plugin.set_param(descriptor.id, value);
        }
    }
    Some(plugin)
}

fn fit<'a, K: Ord + Copy>(
    slots: &'a mut BTreeMap<K, Vst3Slot>,
    key: K,
    request: &Request<'_>,
    prepare: &PrepareContext,
    retiring: &mut Vec<Vst3Plugin>,
) -> Option<&'a mut Vst3Slot> {
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
                });
            }
            Err(error) => {
                log::warn!(
                    "cannot load VST3 `{}` from `{}`: {error}",
                    request.class_id,
                    request.file.display()
                );
                return None;
            }
        }
    }
    slots.get_mut(&key)
}

fn restore(slot: &mut Vst3Slot, state: &PluginState) {
    if slot.needs_state_restore {
        if let Some(bytes) = state.hosted_bytes() {
            let _ = slot.plugin.load_state(&bytes);
        }
        slot.needs_state_restore = false;
    }
    for descriptor in slot.plugin.parameters() {
        if let Some(value) = state.params.get(descriptor.key.as_ref()) {
            let _ = slot.plugin.set_param(descriptor.id, *value);
        }
    }
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
                    file: slot.file.as_ref()?.resolve(None)?,
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
                    file: instrument.file.as_ref()?.resolve(None)?,
                    class_id: class_id.to_string(),
                    state: &instrument.instrument_state,
                },
            ))
        })
        .collect()
}

impl Session {
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
