//! The strip: its effect chain, its parameters, and the lanes that drive them.
//!
//! Three groups that are really one subject, because they all address the same thing through the
//! same name. A [`ParamTarget`] is a fader, a pan, a send level, an instrument parameter or an
//! effect parameter, and everything below resolves one: an effect command finds the chain it sits
//! in, [`Session::set_param`] finds the plugin that owns it, and an automation command finds the
//! descriptor that says what values it may take.
//!
//! Automation is a parameter's value over the timeline, beside the value it is set to. The two do
//! not compete: a target with no lane keeps its static value and only an existing lane takes over,
//! which is what lets a mix be automated one control at a time.
//!
//! Unlike the tempo and the meter, none of these snap. A tempo change is aimed at a place in
//! the song; an automation point is aimed at a moment in the sound, and a filter sweep that
//! could only begin on a sixteenth is a filter sweep with a stutter in it. The frontend is
//! where a grid is offered, through the modifier every other drag already answers to.

use std::sync::Arc;

use auris_core::automation::{Automation, AutomationCurve};
use auris_core::param::{ParamDescriptor, ParamUnit};
use auris_core::plugin::PluginState;
use auris_core::time::Ticks;
use auris_core::{EffectSlotId, TrackId};
use auris_engine::EngineCommand;

use crate::error::SessionError;
use crate::history::Edit;
use crate::param::ParamTarget;

use super::Session;

impl Session {
    /// Adds an effect to a track's chain, or to the master bus when `track` is `None`.
    pub fn add_effect(
        &mut self,
        track: Option<TrackId>,
        effect_id: &str,
    ) -> Result<EffectSlotId, SessionError> {
        if !self.registry.has_effect(effect_id) {
            return Err(SessionError::UnknownPlugin(effect_id.to_string()));
        }
        if let Some(id) = track {
            self.require_track(id)?;
        }
        self.record(Edit::AddEffect);
        let slot = self
            .project
            .add_effect(track, effect_id)
            .ok_or_else(|| SessionError::UnknownTrack(track.map_or(0, |t| t.0)))?;
        self.invalidate_graph();
        Ok(slot)
    }

    /// Removes an effect from wherever it is.
    pub fn remove_effect(&mut self, slot: EffectSlotId) {
        // Look before recording: a slot that is already gone — a double-click, a stale menu —
        // must not cost a redo stack and a snapshot of nothing.
        let exists = std::iter::once(&self.project.master)
            .chain(self.project.tracks.iter().map(|track| &track.mixer))
            .any(|strip| strip.effects.iter().any(|s| s.id == slot));
        if !exists {
            return;
        }
        self.record(Edit::RemoveEffect);
        self.project.remove_effect(slot);
        self.invalidate_graph();
    }

    /// Whether an effect slot is enabled, or `None` when the slot does not exist.
    pub fn effect_enabled(&self, track: Option<TrackId>, slot: EffectSlotId) -> Option<bool> {
        self.strip(track)?
            .effects
            .iter()
            .find(|s| s.id == slot)
            .map(|s| s.enabled)
    }

    /// Bypasses or re-enables an effect.
    pub fn set_effect_enabled(
        &mut self,
        track: Option<TrackId>,
        slot: EffectSlotId,
        enabled: bool,
    ) {
        if self.effect_enabled(track, slot).is_none() {
            return;
        }
        self.record(Edit::BypassEffect);
        if let Some(strip) = self.strip_mut(track)
            && let Some(effect) = strip.effects.iter_mut().find(|s| s.id == slot)
        {
            effect.enabled = enabled;
        }
        self.invalidate_graph();
    }

    /// Moves an effect along its chain by `delta` positions.
    pub fn move_effect(&mut self, track: Option<TrackId>, slot: EffectSlotId, delta: isize) {
        let found = self
            .strip(track)
            .and_then(|strip| strip.effects.iter().position(|s| s.id == slot));
        if found.is_none() {
            return;
        }
        self.record(Edit::ReorderEffects);
        if let Some(strip) = self.strip_mut(track)
            && let Some(index) = strip.effects.iter().position(|s| s.id == slot)
        {
            let target = (index as isize + delta).clamp(0, strip.effects.len() as isize - 1);
            let effect = strip.effects.remove(index);
            strip.effects.insert(target as usize, effect);
        }
        self.invalidate_graph();
    }

    fn strip_mut(&mut self, track: Option<TrackId>) -> Option<&mut auris_core::MixerStrip> {
        match track {
            Some(id) => self.project.track_mut(id).map(|t| &mut t.mixer),
            None => Some(&mut self.project.master),
        }
    }

    /// Parameter descriptors for a plugin, built once by instantiating it.
    pub fn param_descriptors(&mut self, plugin_id: &str) -> Arc<Vec<ParamDescriptor>> {
        if let Some(cached) = self.param_cache.get(plugin_id) {
            return Arc::clone(cached);
        }
        let descriptors = self
            .registry
            .create_instrument(plugin_id)
            .map(|plugin| plugin.parameters().to_vec())
            .or_else(|_| {
                self.registry
                    .create_effect(plugin_id)
                    .map(|plugin| plugin.parameters().to_vec())
            })
            .unwrap_or_default();
        let descriptors = Arc::new(descriptors);
        self.param_cache
            .insert(plugin_id.to_string(), Arc::clone(&descriptors));
        descriptors
    }

    /// The descriptor describing a target, including the mixer's own controls.
    ///
    /// Gain and pan are not plugin parameters, but giving them descriptors lets a frontend
    /// render and edit them with exactly the same code as everything else.
    pub fn descriptor_for(&mut self, target: ParamTarget) -> Option<ParamDescriptor> {
        if let Some(builtin) = Self::mixer_descriptor(target) {
            return Some(builtin);
        }
        // A hosted plugin is not in the registry and never will be, so its parameters can only be
        // had from the instance itself — which is why they are looked up by *slot* rather than by
        // plugin id: two slots may hold the same plugin from two different files.
        if let ParamTarget::Effect { slot, param, .. } = target {
            let hosted = self.hosted_parameters(slot);
            if !hosted.is_empty() {
                return hosted.get(param.index()).cloned();
            }
        }
        if let ParamTarget::Instrument { track, param } = target {
            let hosted = self.hosted_instrument_parameters(track);
            if !hosted.is_empty() {
                return hosted.get(param.index()).cloned();
            }
        }
        let plugin_id = self.plugin_id_for(target)?;
        let index = match target {
            ParamTarget::Instrument { param, .. } | ParamTarget::Effect { param, .. } => {
                param.index()
            }
            _ => return None,
        };
        self.param_descriptors(&plugin_id).get(index).cloned()
    }

    /// The synthesised descriptor for a mixer control, or `None` for a plugin parameter.
    ///
    /// Separate from [`Self::descriptor_for`] because these need no parameter cache, so a
    /// caller holding the session immutably — a render pass building a fader — can still get one.
    pub fn mixer_descriptor(target: ParamTarget) -> Option<ParamDescriptor> {
        match target {
            ParamTarget::TrackPan(_) | ParamTarget::MasterPan => Some(
                ParamDescriptor::new(0u32, "pan", "Pan", -1.0, 1.0, 0.0).with_unit(ParamUnit::Pan),
            ),
            ParamTarget::TrackGain(_) | ParamTarget::MasterGain => Some(ParamDescriptor::decibels(
                0u32, "gain", "Volume", -60.0, 12.0, 0.0,
            )),
            // A send has no headroom above unity: it is how much of a track goes somewhere, and
            // more of it than there is would be a gain stage wearing a send's name.
            ParamTarget::Send { .. } => Some(ParamDescriptor::decibels(
                0u32, "send", "Send", -60.0, 0.0, 0.0,
            )),
            _ => None,
        }
    }

    /// Current value of a parameter, falling back to its default.
    pub fn param_value(&self, target: ParamTarget, descriptor: &ParamDescriptor) -> f32 {
        let from_state = |state: &PluginState| {
            state
                .params
                .get(descriptor.key.as_ref())
                .copied()
                .unwrap_or(descriptor.default)
        };
        match target {
            ParamTarget::TrackGain(id) => self.project.track(id).map_or(0.0, |t| t.mixer.gain_db),
            ParamTarget::TrackPan(id) => self.project.track(id).map_or(0.0, |t| t.mixer.pan),
            ParamTarget::MasterGain => self.project.master.gain_db,
            ParamTarget::MasterPan => self.project.master.pan,
            ParamTarget::Send { track, send } => self
                .project
                .track(track)
                .and_then(|track| track.sends.iter().find(|existing| existing.id == send))
                .map_or(descriptor.default, |send| send.level_db),
            ParamTarget::Instrument { track, .. } => self
                .project
                .track(track)
                .and_then(|t| t.kind.as_instrument())
                .map_or(descriptor.default, |inner| {
                    from_state(&inner.instrument_state)
                }),
            ParamTarget::Effect { track, slot, .. } => self
                .strip(track)
                .and_then(|strip| strip.effects.iter().find(|s| s.id == slot))
                .map_or(descriptor.default, |s| from_state(&s.state)),
        }
    }

    /// Writes a parameter to the document and forwards it to the audio thread.
    ///
    /// Recorded like any other edit. Dragging a knob opens a transaction first, so a sweep is
    /// still one step; every other way of reaching a parameter — a menu choice, a toggle, the
    /// wheel — has no gesture around it and was going unrecorded, which made Undo take back the
    /// edit *before* the parameter change instead.
    pub fn set_param(&mut self, target: ParamTarget, value: f32) {
        // A value that is not a number is not stored: NaN slips every clamp downstream, and
        // `serde_json` writes a non-finite float as `null` — a saved project that can never be
        // opened again. The shipped frontends already sanitise their inputs; this layer is the
        // one that owns the promise, for whichever caller comes next.
        if !value.is_finite() {
            return;
        }
        // Each arm looks its target up before recording, so a stale id costs nothing — and the
        // record carries the target, because coalescing compares edits: two wheel notches on
        // *different* controls within the window must not fold into one step.
        match target {
            ParamTarget::TrackGain(id) => {
                let Ok(index) = self.require_track(id) else {
                    return;
                };
                self.record_repeating(Edit::AdjustParameter(target));
                self.project.tracks[index].mixer.gain_db = value;
                self.send(EngineCommand::SetTrackGain {
                    index,
                    gain_db: value,
                });
            }
            ParamTarget::TrackPan(id) => {
                let Ok(index) = self.require_track(id) else {
                    return;
                };
                self.record_repeating(Edit::AdjustParameter(target));
                self.project.tracks[index].mixer.pan = value;
                self.send(EngineCommand::SetTrackPan { index, pan: value });
            }
            ParamTarget::MasterGain => {
                self.record_repeating(Edit::AdjustParameter(target));
                self.project.master.gain_db = value;
                self.send(EngineCommand::SetMasterGain(value));
            }
            ParamTarget::MasterPan => {
                self.record_repeating(Edit::AdjustParameter(target));
                self.project.master.pan = value;
                self.send(EngineCommand::SetMasterPan(value));
            }
            // Through the typed command rather than repeating its body: a send that has gone is
            // an error there and a silent no-op here, and only one of the two knows how to write
            // the value.
            ParamTarget::Send { track, send } => {
                let _ = self.set_send_level(track, send, value);
            }
            ParamTarget::Instrument { track, param } => {
                let Ok(index) = self.require_track(track) else {
                    return;
                };
                let Some(key) = self.param_key(target) else {
                    return;
                };
                if self.project.tracks[index].kind.as_instrument().is_none() {
                    return;
                }
                self.record_repeating(Edit::AdjustParameter(target));
                if let Some(inner) = self.project.tracks[index].kind.as_instrument_mut() {
                    inner.instrument_state.params.insert(key, value);
                }
                self.send(EngineCommand::SetInstrumentParam {
                    track: index,
                    param,
                    value,
                });
            }
            ParamTarget::Effect { track, slot, param } => {
                let Some(key) = self.param_key(target) else {
                    return;
                };
                let track_index = match track {
                    Some(id) => match self.require_track(id) {
                        Ok(index) => Some(index),
                        Err(_) => return,
                    },
                    None => None,
                };
                let Some(slot_index) = self
                    .strip(track)
                    .and_then(|strip| strip.effects.iter().position(|s| s.id == slot))
                else {
                    return;
                };
                self.record_repeating(Edit::AdjustParameter(target));
                if let Some(strip) = self.strip_mut(track) {
                    strip.effects[slot_index].state.params.insert(key, value);
                }
                self.send(EngineCommand::SetEffectParam {
                    track: track_index,
                    slot: slot_index,
                    param,
                    value,
                });
            }
        }
    }

    /// Every automated parameter in the document.
    pub fn automation(&self) -> &Automation {
        &self.project.automation
    }

    /// Whether `target` is driven by a lane rather than by its stored value.
    ///
    /// A frontend asks before letting a fader be dragged: moving one that automation is about to
    /// overwrite looks like a control that does not work.
    pub fn is_automated(&self, target: ParamTarget) -> bool {
        self.project.automation.lane(target).is_some()
    }

    /// The value driving `target` at `at`, or `None` when it is not automated.
    pub fn automated_value(&self, target: ParamTarget, at: Ticks) -> Option<f32> {
        self.project.automation.value_at(target, at.max_zero())
    }

    /// Writes a point on `target`'s lane, starting the lane if it had none.
    ///
    /// The value is clamped by the parameter's own descriptor, which is also what snaps a
    /// discrete one onto a step: a lane is written in the parameter's units, so a point outside
    /// its range is a point the plugin would refuse anyway.
    ///
    /// Returns whether anything changed, which is `false` for a target this document does not
    /// have and for a point identical to the one already there.
    pub fn set_automation_point(&mut self, target: ParamTarget, at: Ticks, value: f32) -> bool {
        let Some(descriptor) = self.automatable(target) else {
            return false;
        };
        let value = descriptor.clamp(value);
        let curve = curve_for(&descriptor);
        let at = at.max_zero();
        let mut probe = self.project.automation.clone();
        if !probe.set_point(target, curve, at, value) || probe == self.project.automation {
            return false;
        }
        self.record(Edit::WriteAutomation(target));
        self.project.automation = probe;
        self.invalidate_graph();
        true
    }

    /// Moves a point along its lane, taking a new value with it.
    ///
    /// Returns where it landed, which is not always where it was asked to go: dropped onto
    /// another point it replaces that one, since one instant cannot hold two values. A drag wants
    /// [`Self::begin_transaction`] around the whole gesture, the way every other drag does.
    pub fn move_automation_point(
        &mut self,
        target: ParamTarget,
        from: Ticks,
        to: Ticks,
        value: f32,
    ) -> Option<Ticks> {
        let descriptor = self.automatable(target)?;
        let value = descriptor.clamp(value);
        let mut probe = self.project.automation.clone();
        let landed = probe.move_point(target, from, to.max_zero(), value)?;
        if probe == self.project.automation {
            return Some(landed);
        }
        self.record(Edit::WriteAutomation(target));
        self.project.automation = probe;
        self.invalidate_graph();
        Some(landed)
    }

    /// Removes one point, and the lane with it when that was the last one.
    ///
    /// A lane holding nothing is not an empty lane, it is no lane: the parameter goes back to
    /// the value stored on its strip or in its plugin state.
    pub fn remove_automation_point(&mut self, target: ParamTarget, at: Ticks) -> bool {
        let mut probe = self.project.automation.clone();
        if !probe.remove_point(target, at) {
            return false;
        }
        self.record(Edit::EraseAutomation);
        self.project.automation = probe;
        self.invalidate_graph();
        true
    }

    /// Removes a whole lane, giving the parameter its stored value back.
    pub fn clear_automation(&mut self, target: ParamTarget) -> bool {
        let mut probe = self.project.automation.clone();
        if !probe.remove_lane(target) {
            return false;
        }
        self.record(Edit::ClearAutomation);
        self.project.automation = probe;
        self.invalidate_graph();
        true
    }

    /// Changes how an existing lane gets between its points.
    pub fn set_automation_curve(&mut self, target: ParamTarget, curve: AutomationCurve) -> bool {
        let mut probe = self.project.automation.clone();
        if !probe.set_curve(target, curve) || probe == self.project.automation {
            return false;
        }
        self.record(Edit::WriteAutomation(target));
        self.project.automation = probe;
        self.invalidate_graph();
        true
    }

    /// The descriptor for a target this document can actually automate.
    ///
    /// `None` for one it does not have. [`Self::descriptor_for`] answers for a track id nobody
    /// ever created, because a fader's descriptor is synthesised rather than looked up — so the
    /// existence check has to be made here, or a lane could be written into thin air and then
    /// dropped again by the graph builder without anyone being told.
    fn automatable(&mut self, target: ParamTarget) -> Option<ParamDescriptor> {
        let present = match target {
            ParamTarget::MasterGain | ParamTarget::MasterPan => true,
            ParamTarget::TrackGain(id) | ParamTarget::TrackPan(id) => {
                self.project.track(id).is_some()
            }
            ParamTarget::Send { track, send } => self
                .project
                .track(track)
                .is_some_and(|track| track.sends.iter().any(|existing| existing.id == send)),
            ParamTarget::Instrument { track, .. } => self.project.track(track).is_some(),
            ParamTarget::Effect { track, slot, .. } => self
                .strip(track)
                .is_some_and(|strip| strip.effects.iter().any(|effect| effect.id == slot)),
        };
        present.then(|| self.descriptor_for(target)).flatten()
    }

    fn strip(&self, track: Option<TrackId>) -> Option<&auris_core::MixerStrip> {
        match track {
            Some(id) => self.project.track(id).map(|t| &t.mixer),
            None => Some(&self.project.master),
        }
    }

    fn plugin_id_for(&self, target: ParamTarget) -> Option<String> {
        match target {
            ParamTarget::Instrument { track, .. } => Some(
                self.project
                    .track(track)?
                    .kind
                    .as_instrument()?
                    .instrument_id
                    .clone(),
            ),
            ParamTarget::Effect { track, slot, .. } => Some(
                self.strip(track)?
                    .effects
                    .iter()
                    .find(|s| s.id == slot)?
                    .effect_id
                    .clone(),
            ),
            _ => None,
        }
    }

    /// The key a target's parameter is stored under in the document.
    ///
    /// Through [`Self::descriptor_for`], which is the one place that knows a hosted plugin can
    /// only be asked by slot. It used to go to the registry itself, and the registry answering
    /// "no such plugin" for a CLAP effect meant there was no key to write under — so every drag
    /// on one of its sliders was computed, clamped and then thrown away, which looks exactly like
    /// a control that does not work.
    fn param_key(&mut self, target: ParamTarget) -> Option<String> {
        self.descriptor_for(target)
            .map(|descriptor| descriptor.key.to_string())
    }
}

/// How a new lane over a parameter should get between its points.
///
/// A parameter with discrete positions holds; everything else runs in a straight line.
/// Interpolating a chooser would sweep through every option between two settings and sound all of
/// them on the way — a filter opening is a gesture, a waveform changing is not.
///
/// Only consulted when a lane is created. Changing an existing one is
/// [`Session::set_automation_curve`], so writing a point cannot restyle a curve somebody shaped.
fn curve_for(descriptor: &ParamDescriptor) -> AutomationCurve {
    match descriptor.steps {
        Some(_) => AutomationCurve::Hold,
        None => AutomationCurve::Linear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{Scratch, session, undo_depth};
    use auris_core::param::ParamId;

    #[test]
    fn parameters_round_trip_through_the_document() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let target = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        let descriptor = session.descriptor_for(target).unwrap();

        let value = descriptor.clamp(descriptor.max);
        session.set_param(target, value);
        assert_eq!(session.param_value(target, &descriptor), value);

        // The value must land in the saved state under the descriptor's stable key.
        let state = &session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state;
        assert_eq!(state.params.get(descriptor.key.as_ref()), Some(&value));
    }

    #[test]
    fn mixer_controls_get_synthesised_descriptors() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();

        let gain = session
            .descriptor_for(ParamTarget::TrackGain(track))
            .unwrap();
        assert_eq!(gain.format(0.0), "+0.0 dB");
        session.set_param(ParamTarget::TrackGain(track), -6.0);
        assert_eq!(session.project().track(track).unwrap().mixer.gain_db, -6.0);

        let pan = session.descriptor_for(ParamTarget::MasterPan).unwrap();
        assert_eq!(pan.format(0.0), "C");
        session.set_param(ParamTarget::MasterPan, 1.0);
        assert_eq!(session.project().master.pan, 1.0);
    }

    #[test]
    fn a_lane_takes_over_a_parameter_and_giving_it_up_hands_it_back() {
        // The whole contract in one test: no lane means no answer and the stored value stands,
        // a lane answers, and removing the last point is removing the lane.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        assert_eq!(session.automated_value(fader, Ticks::ZERO), None);
        assert!(!session.is_automated(fader));

        assert!(session.set_automation_point(fader, Ticks::ZERO, -6.0));
        assert!(session.is_automated(fader));
        assert_eq!(
            session.automated_value(fader, Ticks::from_beats(9.0)),
            Some(-6.0)
        );

        assert!(session.remove_automation_point(fader, Ticks::ZERO));
        assert!(!session.is_automated(fader));
        assert_eq!(session.automated_value(fader, Ticks::ZERO), None);
    }

    #[test]
    fn a_lane_reads_between_its_points() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.set_automation_point(fader, Ticks::ZERO, -12.0);
        session.set_automation_point(fader, Ticks::from_beats(8.0), 0.0);
        assert_eq!(
            session.automated_value(fader, Ticks::from_beats(4.0)),
            Some(-6.0)
        );
    }

    #[test]
    fn a_written_value_is_clamped_by_the_parameter_it_drives() {
        // A lane is written in the parameter's own units, so a point outside its range is a point
        // the plugin would refuse anyway — better stored as what will actually be heard.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.set_automation_point(fader, Ticks::ZERO, 500.0);
        let written = session.automated_value(fader, Ticks::ZERO).expect("a lane");
        assert!(
            written <= 12.0,
            "the fader tops out at +12 dB, wrote {written}"
        );
    }

    #[test]
    fn a_discrete_parameter_gets_a_lane_that_holds() {
        // Interpolating a chooser would sweep through every option between two settings and sound
        // all of them. Which curve a lane gets is decided where the descriptor is legible.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let waveform = session
            .param_descriptors(
                &session.project().tracks[0]
                    .kind
                    .as_instrument()
                    .unwrap()
                    .instrument_id
                    .clone(),
            )
            .iter()
            .position(|descriptor| descriptor.steps.is_some())
            .map(|index| ParamTarget::Instrument {
                track,
                param: ParamId(index as u32),
            });
        let Some(chooser) = waveform else {
            panic!("the default instrument has no discrete parameter to test with");
        };
        session.set_automation_point(chooser, Ticks::ZERO, 0.0);
        session.set_automation_point(chooser, Ticks::from_beats(8.0), 2.0);
        assert_eq!(
            session.automation().lane(chooser).map(|lane| lane.curve),
            Some(AutomationCurve::Hold)
        );
        assert_eq!(
            session.automated_value(chooser, Ticks::from_beats(4.0)),
            Some(0.0),
            "a chooser holds rather than passing through what is between"
        );
    }

    #[test]
    fn a_lane_cannot_be_written_into_thin_air() {
        // A fader's descriptor is synthesised rather than looked up, so it answers for a track id
        // nobody ever created; without an existence check a lane would be written and then
        // silently dropped by the graph builder.
        let mut session = session();
        assert!(!session.set_automation_point(
            ParamTarget::TrackGain(TrackId(9_999)),
            Ticks::ZERO,
            -6.0
        ));
        assert!(session.automation().is_empty());
    }

    #[test]
    fn every_automation_command_is_one_undo_step_and_only_when_it_changed_something() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.forget_history();

        session.set_automation_point(fader, Ticks::ZERO, -6.0);
        assert_eq!(undo_depth(&mut session), 1);
        // The same point again is not an edit.
        session.set_automation_point(fader, Ticks::ZERO, -6.0);
        assert_eq!(undo_depth(&mut session), 1);
        // Nor is removing a point that was never there.
        assert!(!session.remove_automation_point(fader, Ticks::from_beats(4.0)));
        assert_eq!(undo_depth(&mut session), 1);

        session.set_automation_point(fader, Ticks::from_beats(4.0), 0.0);
        assert_eq!(session.undo(), Some(Edit::WriteAutomation(fader)));
        assert_eq!(
            session.automation().lane(fader).map(|l| l.points().len()),
            Some(1)
        );
    }

    #[test]
    fn a_drag_across_a_lane_is_one_undo_step() {
        // The mechanism every other drag uses: the transaction is opened by the gesture, so the
        // fifty points a pointer writes on the way collapse into the one it landed on.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.set_automation_point(fader, Ticks::ZERO, -6.0);
        session.forget_history();

        session.begin_transaction(Edit::WriteAutomation(fader));
        let mut at = Ticks::ZERO;
        for step in 1..=20 {
            at = session
                .move_automation_point(fader, at, Ticks(step * 48), -6.0 + step as f32 * 0.1)
                .expect("the point is there to move");
        }
        session.end_transaction();
        assert_eq!(undo_depth(&mut session), 1);
    }

    #[test]
    fn deleting_a_track_takes_its_lanes_with_it() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session.set_automation_point(ParamTarget::TrackGain(track), Ticks::ZERO, -6.0);
        session.set_automation_point(ParamTarget::MasterGain, Ticks::ZERO, -3.0);
        session.remove_track(track).unwrap();
        assert_eq!(session.automation().len(), 1);
        assert!(session.automation().lane(ParamTarget::MasterGain).is_some());
    }

    #[test]
    fn a_lane_survives_a_save_and_an_open() {
        let scratch = Scratch::new("automation-round-trip");
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let fader = ParamTarget::TrackGain(track);
        session.set_automation_point(fader, Ticks::ZERO, -12.0);
        session.set_automation_point(fader, Ticks::from_beats(8.0), 0.0);
        let report = session.save_as(&scratch.join("Automated.auris")).unwrap();

        let mut reopened = self::tests::session();
        reopened.open(&report.document).unwrap();
        let fader = ParamTarget::TrackGain(reopened.project().tracks[0].id);
        assert_eq!(
            reopened.automated_value(fader, Ticks::from_beats(4.0)),
            Some(-6.0),
            "the curve has to survive the round trip, not just the points"
        );
    }
}
