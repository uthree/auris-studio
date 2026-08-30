//! What plays, and where its audio goes.
//!
//! Adding, removing, reordering and renaming a track; its colour, height, mute and solo; the
//! instrument or the preset it plays; the bus its output feeds and the sends that tap it on the
//! way. A bus is a [`TrackKind`](auris_core::TrackKind) rather than a thing of its own, so every
//! command here that addresses a strip by [`TrackId`] addresses a bus too — which is why routing
//! lives with tracks instead of with the mixer.
//!
//! The three `require_*` helpers at the end are how everything above refuses an id nothing owns
//! *before* it records an undo step; [`Session::require_track`] is shared with the files that
//! address a track from the other end.

use auris_core::plugin::{PluginKind, PluginState};
use auris_core::{AuxSend, Color, Output, PresetRef, SendId, Track, TrackId};
use auris_engine::EngineCommand;
use auris_sampler::{SAMPLER_ID, store_preset, stored_preset};

use crate::error::SessionError;
use crate::history::Edit;
use crate::param::ParamTarget;

use super::Session;

/// Shortest a lane may be made, in pixels.
///
/// A lane still has to hold a name and the buttons beside it; below this the header stops being
/// something anybody can hit.
pub const MIN_TRACK_HEIGHT: f32 = 24.0;

/// Tallest a lane may be made, in pixels.
///
/// Not a limit anyone reaches on purpose — it is the guard against a drag that ran away, and
/// against a document that arrived with a nonsense number in it.
pub const MAX_TRACK_HEIGHT: f32 = 400.0;

impl Session {
    /// Appends an instrument track.
    pub fn add_instrument_track(
        &mut self,
        name: impl Into<String>,
        instrument_id: &str,
    ) -> Result<TrackId, SessionError> {
        if !self.registry.has_instrument(instrument_id) {
            return Err(SessionError::UnknownPlugin(instrument_id.to_string()));
        }
        self.record(Edit::AddInstrumentTrack);
        let id = self.project.add_instrument_track(name, instrument_id);
        self.invalidate_graph();
        Ok(id)
    }

    /// Appends an instrument track playing whatever the registry nominates as its default.
    pub fn add_default_instrument_track(
        &mut self,
        name: impl Into<String>,
    ) -> Result<TrackId, SessionError> {
        let instrument = self
            .registry
            .default_instrument_id()
            .ok_or_else(|| SessionError::UnknownPlugin("<any instrument>".into()))?
            .to_string();
        self.add_instrument_track(name, &instrument)
    }

    /// Appends an audio track.
    pub fn add_audio_track(&mut self, name: impl Into<String>) -> TrackId {
        self.record(Edit::AddAudioTrack);
        let id = self.project.add_audio_track(name);
        self.invalidate_graph();
        id
    }

    /// Appends a bus: a mixing point that nothing is routed to yet.
    pub fn add_bus_track(&mut self, name: impl Into<String>) -> TrackId {
        self.record(Edit::AddBusTrack);
        let id = self.project.add_bus_track(name);
        self.invalidate_graph();
        id
    }

    /// Points a track's output at a bus, or back at the master.
    ///
    /// Refused when the destination is not a bus, or when the route would make a signal loop back
    /// on itself. Both are checked before anything is recorded: a command that pushes an undo step
    /// and then fails has cost the user a rung that reverses nothing and a redo branch that is
    /// simply gone.
    pub fn set_track_output(&mut self, id: TrackId, output: Output) -> Result<(), SessionError> {
        self.require_track(id)?;
        if let Some(bus) = output.bus() {
            self.require_bus(bus)?;
            if self.project.routing_would_cycle(id, bus) {
                return Err(SessionError::RoutingLoop {
                    from: id.0,
                    to: bus.0,
                });
            }
        }
        if self.project.track(id).is_some_and(|t| t.output == output) {
            return Ok(());
        }
        self.record(Edit::SetTrackOutput);
        if let Some(track) = self.project.track_mut(id) {
            track.output = output;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Adds a post-fader send at unity from `id` to `bus`, returning its new id.
    ///
    /// Unity because a send is added in order to be heard: starting it at silence would make the
    /// first thing every user does be to undo the default.
    pub fn add_send(&mut self, id: TrackId, bus: TrackId) -> Result<SendId, SessionError> {
        self.require_track(id)?;
        self.require_bus(bus)?;
        if self.project.routing_would_cycle(id, bus) {
            return Err(SessionError::RoutingLoop {
                from: id.0,
                to: bus.0,
            });
        }
        self.record(Edit::AddSend);
        let send = self.project.next_send_id();
        if let Some(track) = self.project.track_mut(id) {
            track.sends.push(AuxSend::new(send, bus));
        }
        self.invalidate_graph();
        Ok(send)
    }

    /// Removes a send from a track.
    pub fn remove_send(&mut self, id: TrackId, send: SendId) -> Result<(), SessionError> {
        self.require_send(id, send)?;
        self.record(Edit::RemoveSend);
        self.project.remove_send(id, send);
        self.invalidate_graph();
        Ok(())
    }

    /// Turns a send's level, in decibels.
    ///
    /// A knob rather than a structural edit, so the change travels as a command and the graph is
    /// left where it is. The undo step is [`Edit::AdjustParameter`] over
    /// [`ParamTarget::Send`], the same one [`Self::set_param`] records — so a drag on one send
    /// folds into a single step whichever of the two paths the frontend reached it by, and
    /// turning a *different* send starts a new one.
    pub fn set_send_level(
        &mut self,
        id: TrackId,
        send: SendId,
        level_db: f32,
    ) -> Result<(), SessionError> {
        let (index, position) = self.require_send(id, send)?;
        if !level_db.is_finite() {
            return Err(SessionError::NotFinite(level_db as f64));
        }
        self.record_repeating(Edit::AdjustParameter(ParamTarget::Send { track: id, send }));
        self.project.tracks[index].sends[position].level_db = level_db;
        self.send(EngineCommand::SetSendLevel {
            track: index,
            send: position,
            level_db,
        });
        Ok(())
    }

    /// Moves a send's tap before or after the track's fader.
    ///
    /// Where the copy is taken from is part of the graph's shape rather than a value in it, so
    /// unlike the level this rebuilds.
    pub fn set_send_pre_fader(
        &mut self,
        id: TrackId,
        send: SendId,
        pre_fader: bool,
    ) -> Result<(), SessionError> {
        let (index, position) = self.require_send(id, send)?;
        if self.project.tracks[index].sends[position].pre_fader == pre_fader {
            return Ok(());
        }
        self.record(Edit::SetSendPreFader);
        self.project.tracks[index].sends[position].pre_fader = pre_fader;
        self.invalidate_graph();
        Ok(())
    }

    /// Every bus in the project, for a routing picker.
    pub fn buses(&self) -> impl Iterator<Item = &Track> {
        self.project
            .tracks
            .iter()
            .filter(|track| track.kind.is_bus())
    }

    /// `true` when `id` could be routed into `bus` — as an output or through a send — without the
    /// signal looping back on itself.
    ///
    /// The rule a picker should grey a row out by, worked out here rather than in each frontend:
    /// which destinations are legal is a fact about the document, and a UI that decided it for
    /// itself would eventually disagree with the command that has to enforce it.
    pub fn can_route(&self, id: TrackId, bus: TrackId) -> bool {
        self.project
            .track(bus)
            .is_some_and(|track| track.kind.is_bus())
            && bus != id
            && !self.project.routing_would_cycle(id, bus)
    }

    /// The buses `id` could be routed into without making a loop.
    pub fn available_buses(&self, id: TrackId) -> Vec<TrackId> {
        self.buses()
            .map(|bus| bus.id)
            .filter(|bus| self.can_route(id, *bus))
            .collect()
    }

    /// Removes a track.
    pub fn remove_track(&mut self, id: TrackId) -> Result<(), SessionError> {
        self.require_track(id)?;
        self.record(Edit::DeleteTrack);
        self.project.remove_track(id);
        // An arm on a track that no longer exists would refuse the next take rather than being
        // ignored by it, and the button that could clear it has gone with the track.
        self.disarm_track(id);
        self.invalidate_graph();
        Ok(())
    }

    /// Moves a track to a new position in the list, clamping into range.
    ///
    /// Structural, so the graph is rebuilt: the engine addresses tracks by *position*, and every
    /// index in flight would otherwise point one track away from what it named. Nothing the
    /// document holds is addressed that way — automation lanes, a routing output and a send all
    /// name a track by id — so the mix survives the move unchanged.
    pub fn move_track(&mut self, id: TrackId, to_index: usize) -> Result<(), SessionError> {
        let from = self.require_track(id)?;
        let to = to_index.min(self.project.tracks.len().saturating_sub(1));
        if from == to {
            return Ok(());
        }
        self.record(Edit::MoveTrack);
        self.project.move_track(id, to);
        self.invalidate_graph();
        Ok(())
    }

    /// Renames a track.
    pub fn rename_track(
        &mut self,
        id: TrackId,
        name: impl Into<String>,
    ) -> Result<(), SessionError> {
        self.require_track(id)?;
        self.record(Edit::RenameTrack);
        if let Some(track) = self.project.track_mut(id) {
            track.name = name.into();
        }
        Ok(())
    }

    /// Tints a track, and the clips on it.
    ///
    /// A new track picks a palette entry by its position, which is a sensible start and a poor
    /// finish: the order tracks were made in has nothing to do with which of them are drums. This
    /// is what makes the colour a choice. Nothing is heard, so the graph is left alone.
    pub fn set_track_color(&mut self, id: TrackId, color: Color) -> Result<(), SessionError> {
        self.require_track(id)?;
        if self
            .project
            .track(id)
            .is_some_and(|track| track.color == color)
        {
            return Ok(());
        }
        self.record(Edit::SetTrackColor);
        if let Some(track) = self.project.track_mut(id) {
            track.color = color;
        }
        Ok(())
    }

    /// Silences or unsilences a track.
    pub fn set_track_mute(&mut self, id: TrackId, mute: bool) -> Result<(), SessionError> {
        let index = self.require_track(id)?;
        self.record(Edit::MuteTrack);
        self.project.tracks[index].mixer.mute = mute;
        self.send(EngineCommand::SetTrackMute { index, mute });
        Ok(())
    }

    /// Solos or unsolos a track.
    ///
    /// Solo decides which *other* tracks are audible, so unlike mute it cannot be expressed as
    /// one per-track command and the graph is rebuilt instead.
    pub fn set_track_solo(&mut self, id: TrackId, solo: bool) -> Result<(), SessionError> {
        self.require_track(id)?;
        self.record(Edit::SoloTrack);
        if let Some(track) = self.project.track_mut(id) {
            track.mixer.solo = solo;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Replaces a track's instrument, discarding the previous plugin's parameter values and the
    /// automation that drove them.
    pub fn set_track_instrument(
        &mut self,
        id: TrackId,
        instrument_id: &str,
    ) -> Result<(), SessionError> {
        if self.registry.descriptor(instrument_id).map(|d| d.kind) != Some(PluginKind::Instrument) {
            return Err(SessionError::UnknownPlugin(instrument_id.to_string()));
        }
        self.require_track(id)?;
        // An audio track has no instrument to change. This used to fall through the `if let`
        // below, do nothing, and report success — after recording an undo step for the edit it
        // had not made, so the history grew a rung that reversed nothing. The guard belongs here
        // rather than in whichever frontend happens to remember: the invariant is the document's.
        if !self
            .project
            .track(id)
            .is_some_and(|track| track.kind.is_instrument())
        {
            return Err(SessionError::WrongTrackKind {
                id: id.0,
                // The track's own word for itself: a singer track refuses here too, because
                // its preview voice is chosen by its kind rather than by a picker.
                actual: self
                    .project
                    .track(id)
                    .map_or("a track", |track| track.kind.label()),
                expected: "an instrument track",
            });
        }
        self.record(Edit::ChangeInstrument);
        let mut swapped = false;
        if let Some(inner) = self
            .project
            .track_mut(id)
            .and_then(|track| track.kind.as_instrument_mut())
        {
            swapped = inner.instrument_id != instrument_id;
            inner.instrument_id = instrument_id.to_string();
            // The saved values belong to the old plugin; applying them to a different one would
            // write another plugin's numbers into unrelated controls.
            inner.instrument_state = PluginState::empty();
            // And the file with them. This one is not cosmetic: a track still naming a `.clap`
            // while its id is a registry id is a track the session keeps a hosted instance
            // alive for, of a plugin that file does not contain and never will.
            inner.file = None;
        }
        if swapped {
            // And so do the curves that were writing those values every block, for the same
            // reason. After the `record`, so that undo brings the lanes back with the plugin.
            self.project.remove_instrument_automation(id);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Points a track at one of an imported SoundFont's sounds.
    ///
    /// Choosing a sound implies choosing the instrument that makes it, so a track playing
    /// anything else is switched to the sampler as part of the same edit — which is what makes
    /// picking a preset out of a library one gesture rather than two.
    ///
    /// A track already on the sampler keeps its level, reverb and chorus — and the lanes that
    /// drive them: those are how the player is set up, not which sound it is playing, and losing
    /// them every time somebody auditioned a neighbouring preset would be its own small tragedy.
    /// A track arriving from another instrument loses both, because they described that plugin.
    pub fn set_track_preset(&mut self, id: TrackId, preset: PresetRef) -> Result<(), SessionError> {
        self.require_track(id)?;
        if !self
            .project
            .track(id)
            .is_some_and(|track| track.kind.is_instrument())
        {
            return Err(SessionError::WrongTrackKind {
                id: id.0,
                // The track's own word for itself: a singer track refuses here too, because
                // its preview voice is chosen by its kind rather than by a picker.
                actual: self
                    .project
                    .track(id)
                    .map_or("a track", |track| track.kind.label()),
                expected: "an instrument track",
            });
        }
        if !self.project.soundfonts.contains_key(&preset.font) {
            return Err(SessionError::UnknownSoundFont(preset.font.0));
        }
        self.record(Edit::ChoosePreset);
        let mut swapped = false;
        if let Some(inner) = self
            .project
            .track_mut(id)
            .and_then(|track| track.kind.as_instrument_mut())
        {
            if inner.instrument_id != SAMPLER_ID {
                inner.instrument_id = SAMPLER_ID.to_string();
                inner.instrument_state = PluginState::empty();
                swapped = true;
            }
            store_preset(&mut inner.instrument_state, preset);
        }
        if swapped {
            // The track was playing something else a moment ago, so its lanes were addressed to
            // that plugin's parameters. After the `record`, so that undo brings them back.
            self.project.remove_instrument_automation(id);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Points a track at a General MIDI sound out of the shipped library, adopting the font
    /// into the project when it is not referenced yet.
    ///
    /// One undo step for both: a font adopted for a choice that was then taken back is a
    /// reference nothing plays. `bank` and `patch` are the sound's address in the font — a
    /// kit lives in bank 128, a melodic program in bank 0 — and which reading a program
    /// number gets is the caller's decision, made where the part's kind is known.
    pub fn set_track_general_midi(
        &mut self,
        id: TrackId,
        bank: i32,
        patch: i32,
    ) -> Result<(), SessionError> {
        self.require_track(id)?;
        // The kind check `set_track_preset` would make, made before anything is adopted or
        // recorded: a refusal after the adoption would leave the font behind it.
        if !self
            .project
            .track(id)
            .is_some_and(|track| track.kind.is_instrument())
        {
            return Err(SessionError::WrongTrackKind {
                id: id.0,
                actual: self
                    .project
                    .track(id)
                    .map_or("a track", |track| track.kind.label()),
                expected: "an instrument track",
            });
        }
        self.begin_transaction(Edit::ChoosePreset);
        let Some(font) = self.adopt_general_midi_here() else {
            self.end_transaction();
            return Err(SessionError::LibraryMissing);
        };
        let outcome = self.set_track_preset(id, PresetRef { font, bank, patch });
        self.end_transaction();
        outcome
    }

    /// Which SoundFont sound a track plays, or `None` when it plays something else entirely.
    pub fn track_preset(&self, id: TrackId) -> Option<PresetRef> {
        let inner = self.project.track(id)?.kind.as_instrument()?;
        if inner.instrument_id != SAMPLER_ID {
            return None;
        }
        stored_preset(&inner.instrument_state)
    }

    /// Copies a track, its clips and its whole effect chain, below the original.
    pub fn duplicate_track(&mut self, id: TrackId) -> Result<TrackId, SessionError> {
        self.require_track(id)?;
        self.record(Edit::DuplicateTrack);
        let copy = self
            .project
            .duplicate_track(id)
            .ok_or(SessionError::UnknownTrack(id.0))?;
        self.invalidate_graph();
        Ok(copy)
    }

    /// Sets a track's lane height, for frontends that draw one.
    ///
    /// Recorded like every other track property, and for the same two reasons rather than one:
    /// `record` is what puts a step on the undo stack, and it is also what marks the document
    /// dirty. A stored field written without it is a change that cannot be taken back *and* one
    /// that autosave does not know happened — so a lane resized and nothing else touched
    /// afterwards is a lane that is its old height again on the next open.
    pub fn set_track_height(&mut self, id: TrackId, height: f32) -> Result<(), SessionError> {
        self.require_track(id)?;
        // `clamp` would pass a NaN straight into a stored field; the floor is as good an answer
        // as any to a height that is not a number.
        let height = if height.is_finite() {
            height.clamp(MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT)
        } else {
            MIN_TRACK_HEIGHT
        };
        if self
            .project
            .track(id)
            .is_some_and(|track| track.height == height)
        {
            return Ok(());
        }
        self.record(Edit::SetTrackHeight);
        if let Some(track) = self.project.track_mut(id) {
            track.height = height;
        }
        Ok(())
    }

    pub(super) fn require_track(&self, id: TrackId) -> Result<usize, SessionError> {
        self.project
            .track_index(id)
            .ok_or(SessionError::UnknownTrack(id.0))
    }

    /// A track that exists *and* is a mixing point, which is the only thing audio can be sent to.
    fn require_bus(&self, id: TrackId) -> Result<usize, SessionError> {
        let index = self.require_track(id)?;
        match self.project.tracks[index].kind.is_bus() {
            true => Ok(index),
            false => Err(SessionError::NotABus(id.0)),
        }
    }

    /// The track's index and the send's position in its list, both of which the engine addresses
    /// things by.
    fn require_send(&self, id: TrackId, send: SendId) -> Result<(usize, usize), SessionError> {
        let index = self.require_track(id)?;
        let position = self.project.tracks[index]
            .sends
            .iter()
            .position(|existing| existing.id == send)
            .ok_or(SessionError::UnknownSend {
                track: id.0,
                send: send.0,
            })?;
        Ok((index, position))
    }

    /// A unit-range value fit to store: clamped, with non-finite input becoming the floor.
    ///
    /// `clamp` passes NaN through, and this layer owns the promise that what goes into the
    /// document can come back out of it — `serde_json` writes a non-finite float as `null`,
    /// which no `f32` field will ever deserialise again.
    pub(super) fn finite_unit(value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{named_font, session, session_with_clip, undo_depth};
    use auris_core::SoundFontId;
    use auris_core::param::ParamId;
    use auris_core::time::Ticks;

    /// A session holding one instrument track and one bus, with nothing routed yet.
    fn routed_session() -> (Session, TrackId, TrackId) {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let bus = session.add_bus_track("Reverb");
        (session, track, bus)
    }

    #[test]
    fn a_track_moves_up_and_down_the_list_in_one_step() {
        let mut session = session();
        let first = session.add_default_instrument_track("A").expect("track");
        let second = session.add_default_instrument_track("B").expect("track");
        let third = session.add_default_instrument_track("C").expect("track");
        let order = |session: &Session| -> Vec<TrackId> {
            session
                .project
                .tracks
                .iter()
                .map(|track| track.id)
                .collect()
        };

        session.move_track(first, 2).expect("to the end");
        assert_eq!(order(&session), vec![second, third, first]);
        session.undo().expect("a step");
        assert_eq!(order(&session), vec![first, second, third]);

        // Past the end is the end rather than an error: a hand that overshoots means the bottom.
        session.move_track(first, 99).expect("clamped");
        assert_eq!(order(&session), vec![second, third, first]);
        // And a move to where it already is changes nothing and records nothing.
        let before = undo_depth(&mut session);
        session.move_track(first, 2).expect("already there");
        assert_eq!(undo_depth(&mut session), before);
    }

    #[test]
    fn reordering_tracks_leaves_the_routing_alone() {
        // Everything in the document names a track by id, so a bus may end up *above* the tracks
        // feeding it — which is only a fact about the list, not about the mix. The renderer walks
        // the routing order rather than the list, so what is heard does not change.
        let (mut session, kick, bus) = routed_session();
        session.set_track_output(kick, Output::Bus(bus)).unwrap();
        session.set_param(ParamTarget::TrackGain(bus), -6.0);

        session.move_track(bus, 0).expect("the bus goes first");
        assert_eq!(session.project.tracks[0].id, bus);
        assert_eq!(
            session.project.track(kick).unwrap().output,
            Output::Bus(bus)
        );
        assert_eq!(session.project.track(bus).unwrap().mixer.gain_db, -6.0);
        // A bus above its feeders is still rendered after them.
        let order = session.project.routing_order();
        let at = |id: TrackId| {
            let index = session.project.track_index(id).unwrap();
            order.iter().position(|slot| *slot == index).unwrap()
        };
        assert!(at(kick) < at(bus));
    }

    #[test]
    fn a_track_routes_into_a_bus_and_back_out_to_the_master() {
        let (mut session, track, bus) = routed_session();
        session
            .set_track_output(track, Output::Bus(bus))
            .expect("a bus is a legal destination");
        assert_eq!(
            session.project.track(track).unwrap().output,
            Output::Bus(bus)
        );

        // And it is one undo step, which puts the track back on the master.
        session.undo().expect("a step");
        assert_eq!(session.project.track(track).unwrap().output, Output::Master);
    }

    #[test]
    fn only_a_bus_can_be_routed_into() {
        let (mut session, track, _) = routed_session();
        let other = session.add_audio_track("Sample");
        let error = session
            .set_track_output(track, Output::Bus(other))
            .expect_err("an audio track is not a mixing point");
        assert!(matches!(error, SessionError::NotABus(id) if id == other.0));
        assert!(matches!(
            session.add_send(track, other),
            Err(SessionError::NotABus(_))
        ));
    }

    #[test]
    fn a_refused_route_costs_neither_a_step_nor_the_redo_branch() {
        // Validation before `record`, stated as a test: a command that pushes a step and then
        // fails leaves a rung that reverses nothing and throws away whatever could be redone.
        let (mut session, track, bus) = routed_session();
        session.set_track_output(track, Output::Bus(bus)).unwrap();
        session.undo().expect("back to the master");
        // Two steps behind and one ahead: exactly the state a refused command must not disturb.

        let _ = session.set_track_output(track, Output::Bus(TrackId(9_999)));
        let _ = session.add_send(track, TrackId(9_999));

        assert!(session.redo().is_some(), "the redo branch was thrown away");
        assert_eq!(
            session.project.track(track).unwrap().output,
            Output::Bus(bus)
        );
        // And nothing was pushed: one undo is back at the master, with no phantom rung between.
        session.undo().expect("a step");
        assert_eq!(session.project.track(track).unwrap().output, Output::Master);
    }

    #[test]
    fn routing_that_would_loop_is_refused() {
        let (mut session, _, first) = routed_session();
        let second = session.add_bus_track("Delay");
        session
            .set_track_output(first, Output::Bus(second))
            .expect("one bus into another is fine");

        let error = session
            .set_track_output(second, Output::Bus(first))
            .expect_err("that closes the circle");
        assert!(matches!(
            error,
            SessionError::RoutingLoop { from, to } if from == second.0 && to == first.0
        ));
        // A send round the same circle is refused for the same reason, and so is a bus into
        // itself — a loop has no order it can be rendered in either way.
        assert!(matches!(
            session.add_send(second, first),
            Err(SessionError::RoutingLoop { .. })
        ));
        assert!(matches!(
            session.set_track_output(first, Output::Bus(first)),
            Err(SessionError::RoutingLoop { .. })
        ));
    }

    #[test]
    fn what_may_be_routed_where_is_one_rule_the_picker_and_the_command_share() {
        // A frontend greys a row out by this and the command refuses by the same facts, so the
        // list can never offer something the session would then turn down.
        let (mut session, track, first) = routed_session();
        let second = session.add_bus_track("Delay");
        let audio = session.add_audio_track("Sample");
        session
            .set_track_output(first, Output::Bus(second))
            .unwrap();

        assert!(session.can_route(track, first));
        assert!(session.can_route(first, second));
        // Round the circle, into itself, into something that is not a bus, and into a track that
        // was never made.
        assert!(!session.can_route(second, first));
        assert!(!session.can_route(first, first));
        assert!(!session.can_route(track, audio));
        assert!(!session.can_route(track, TrackId(9_999)));

        // And every one of those refusals is the error the command gives back.
        for (from, to) in [(second, first), (first, first)] {
            assert!(matches!(
                session.set_track_output(from, Output::Bus(to)),
                Err(SessionError::RoutingLoop { .. })
            ));
        }
        assert!(matches!(
            session.set_track_output(track, Output::Bus(audio)),
            Err(SessionError::NotABus(_))
        ));
    }

    #[test]
    fn the_buses_offered_are_the_ones_that_would_not_loop() {
        // The list a picker shows is a fact about the document, so it is worked out here rather
        // than in each frontend — one that offered an illegal destination would be offering an
        // error message.
        let (mut session, track, first) = routed_session();
        let second = session.add_bus_track("Delay");
        session
            .set_track_output(first, Output::Bus(second))
            .unwrap();

        assert_eq!(session.available_buses(track), vec![first, second]);
        // `first` already feeds `second`, so `second` cannot feed it back — and no bus can feed
        // itself.
        assert_eq!(session.available_buses(second), Vec::new());
        assert_eq!(session.available_buses(first), vec![second]);
    }

    #[test]
    fn a_send_starts_at_unity_after_the_fader() {
        // A send is added in order to be heard. Starting it at silence would make the first thing
        // every user does be to undo the default.
        let (mut session, track, bus) = routed_session();
        let send = session.add_send(track, bus).expect("a send");
        let added = &session.project.track(track).unwrap().sends[0];
        assert_eq!(added.id, send);
        assert_eq!(added.target, bus);
        assert_eq!(added.level_db, 0.0);
        assert!(!added.pre_fader);
    }

    #[test]
    fn turning_one_send_repeatedly_is_one_step_and_turning_another_is_a_new_one() {
        let (mut session, track, bus) = routed_session();
        let first = session.add_send(track, bus).unwrap();
        let second = session.add_send(track, bus).unwrap();
        let before = undo_depth(&mut session);
        while session.redo().is_some() {}

        for level in [-1.0, -2.0, -3.0] {
            session.set_send_level(track, first, level).unwrap();
        }
        session.set_send_level(track, second, -9.0).unwrap();
        assert_eq!(
            undo_depth(&mut session),
            before + 2,
            "a drag on one send folds; moving to another must not"
        );
    }

    #[test]
    fn a_send_to_a_deleted_bus_leaves_with_it() {
        let (mut session, track, bus) = routed_session();
        session.set_track_output(track, Output::Bus(bus)).unwrap();
        session.add_send(track, bus).unwrap();

        session.remove_track(bus).expect("the bus goes");
        let track = session.project.track(track).unwrap();
        assert_eq!(track.output, Output::Master);
        assert!(track.sends.is_empty());
    }

    #[test]
    fn a_send_that_is_not_there_is_named_rather_than_ignored() {
        let (mut session, track, _) = routed_session();
        let error = session
            .set_send_level(track, SendId(1_234), -6.0)
            .expect_err("no such send");
        assert!(matches!(
            error,
            SessionError::UnknownSend { track: t, send } if t == track.0 && send == 1_234
        ));
    }

    #[test]
    fn a_send_level_that_is_not_a_number_is_refused() {
        // The same promise every stored float carries: `serde_json` writes a non-finite as
        // `null`, and no `f32` field will ever read that back.
        let (mut session, track, bus) = routed_session();
        let send = session.add_send(track, bus).unwrap();
        assert!(matches!(
            session.set_send_level(track, send, f32::NAN),
            Err(SessionError::NotFinite(_))
        ));
        assert_eq!(
            session.project.track(track).unwrap().sends[0].level_db,
            0.0,
            "the refused value must not have landed"
        );
    }

    #[test]
    fn a_new_track_never_starts_on_the_sampler() {
        // The sampler sorts first by plugin id, so before there was a nominated default it won
        // the "first registered instrument" race — and a new track came up silent.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let instrument = session
            .project
            .track(track)
            .and_then(|t| t.kind.as_instrument())
            .map(|inner| inner.instrument_id.clone())
            .expect("an instrument track");
        assert_ne!(instrument, SAMPLER_ID);
        assert_eq!(instrument, crate::registry::DEFAULT_INSTRUMENT);
    }

    #[test]
    fn choosing_a_sound_also_chooses_the_instrument_that_makes_it() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let font = named_font(&mut session, "Orchestra");
        let preset = PresetRef {
            font,
            bank: 0,
            patch: 40,
        };

        session.set_track_preset(track, preset).expect("chosen");

        let inner = session
            .project
            .track(track)
            .and_then(|t| t.kind.as_instrument())
            .expect("an instrument track");
        assert_eq!(inner.instrument_id, SAMPLER_ID);
        assert_eq!(session.track_preset(track), Some(preset));
    }

    #[test]
    fn auditioning_a_second_preset_keeps_how_the_player_is_set_up() {
        // Level, reverb and chorus describe the player, not the sound it is playing. Clearing
        // them every time somebody tried the next preset along would be its own small tragedy.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let font = named_font(&mut session, "Orchestra");
        session
            .set_track_preset(
                track,
                PresetRef {
                    font,
                    bank: 0,
                    patch: 40,
                },
            )
            .expect("chosen");
        if let Some(inner) = session
            .project
            .track_mut(track)
            .and_then(|t| t.kind.as_instrument_mut())
        {
            inner.instrument_state.params.insert("level".into(), -6.0);
        }
        let knob = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        assert!(session.set_automation_point(knob, Ticks::ZERO, 0.0));

        let second = PresetRef {
            font,
            bank: 0,
            patch: 41,
        };
        session.set_track_preset(track, second).expect("chosen");

        let inner = session
            .project
            .track(track)
            .and_then(|t| t.kind.as_instrument())
            .expect("an instrument track");
        assert_eq!(session.track_preset(track), Some(second));
        assert_eq!(inner.instrument_state.params.get("level"), Some(&-6.0));
        // The plugin did not change, so its curves still address exactly what they were drawn
        // for. Dropping them here would lose a sweep to the act of trying the next patch along.
        assert!(
            session.automation().lane(knob).is_some(),
            "an audition is not a change of plugin"
        );
    }

    #[test]
    fn a_preset_from_a_font_the_project_does_not_have_is_refused() {
        // The id would end up in the document and resolve to nothing for the rest of its life.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        session.forget_history();

        let refused = session.set_track_preset(
            track,
            PresetRef {
                font: SoundFontId(999),
                bank: 0,
                patch: 0,
            },
        );
        assert!(matches!(refused, Err(SessionError::UnknownSoundFont(999))));
        assert!(!session.can_undo(), "a refused edit left a step behind");
        assert_eq!(session.track_preset(track), None);
    }

    #[test]
    fn an_audio_track_has_no_sound_to_choose() {
        let mut session = session();
        let audio = session.add_audio_track("Sample");
        let font = named_font(&mut session, "Orchestra");
        session.forget_history();

        let refused = session.set_track_preset(
            audio,
            PresetRef {
                font,
                bank: 0,
                patch: 0,
            },
        );
        assert!(matches!(refused, Err(SessionError::WrongTrackKind { .. })));
        assert!(!session.can_undo(), "a refused edit left a step behind");
    }

    #[test]
    fn a_track_on_another_instrument_reports_no_preset() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        assert_eq!(session.track_preset(track), None);
    }

    #[test]
    fn an_audio_track_has_no_instrument_to_change() {
        // This used to return `Ok` having changed nothing and recorded an undo step anyway, so
        // the history grew a rung that reversed nothing at all.
        let mut session = session();
        let audio = session.add_audio_track("Sample");
        let instrument = session
            .registry()
            .default_instrument_id()
            .expect("the default registry has instruments")
            .to_string();
        session.forget_history();

        let refused = session.set_track_instrument(audio, &instrument);
        assert!(matches!(refused, Err(SessionError::WrongTrackKind { .. })));
        assert!(!session.can_undo(), "a refused edit left a step behind");
    }

    #[test]
    fn unknown_plugin_ids_are_refused_rather_than_stored() {
        let mut session = session();
        let error = session
            .add_instrument_track("Ghost", "nobody.synth.missing")
            .unwrap_err();
        assert!(matches!(error, SessionError::UnknownPlugin(_)));
        assert!(session.project().tracks.is_empty());

        let track = session.add_default_instrument_track("Real").unwrap();
        assert!(matches!(
            session.add_effect(Some(track), "nobody.fx.missing"),
            Err(SessionError::UnknownPlugin(_))
        ));
        assert!(
            session
                .project()
                .track(track)
                .unwrap()
                .mixer
                .effects
                .is_empty()
        );
    }

    #[test]
    fn changing_the_instrument_discards_the_old_plugin_state() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let first = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        session.set_param(first, 1.0);
        assert!(session.set_automation_point(first, Ticks::ZERO, 1.0));

        session
            .set_track_instrument(track, "auris.synth.fm2")
            .unwrap();
        let state = &session
            .project()
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state;
        assert!(
            state.params.is_empty(),
            "another plugin's values must not survive the swap"
        );
        // A lane names the track and the parameter's index, never the plugin, so one left behind
        // would go on sweeping whatever the new instrument keeps at that index.
        assert!(
            session.automation().lane(first).is_none(),
            "another plugin's curve must not survive the swap either"
        );

        // The removal sits after the `record`, so the whole edit comes back together.
        assert_eq!(session.undo(), Some(Edit::ChangeInstrument));
        assert!(
            session.automation().lane(first).is_some(),
            "undo put the instrument back without its automation"
        );
    }

    #[test]
    fn choosing_a_sound_drops_the_lanes_that_drove_the_old_instrument() {
        // Picking a preset switches a track off its synth, which is a change of plugin like any
        // other — and the lanes belonged to the synth.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        let font = named_font(&mut session, "Orchestra");
        let first = ParamTarget::Instrument {
            track,
            param: ParamId(0),
        };
        assert!(session.set_automation_point(first, Ticks::ZERO, 1.0));

        session
            .set_track_preset(
                track,
                PresetRef {
                    font,
                    bank: 0,
                    patch: 40,
                },
            )
            .expect("chosen");
        assert!(
            session.automation().lane(first).is_none(),
            "the synth's curve stayed behind to drive the sampler"
        );

        assert_eq!(session.undo(), Some(Edit::ChoosePreset));
        assert!(
            session.automation().lane(first).is_some(),
            "undo put the instrument back without its automation"
        );
    }

    #[test]
    fn duplicating_a_track_is_one_undo_step() {
        let (mut session, track, _) = session_with_clip();
        let copy = session.duplicate_track(track).unwrap();
        assert_eq!(session.project().tracks.len(), 2);
        assert_ne!(copy, track);

        session.undo().unwrap();
        assert_eq!(session.project().tracks.len(), 1);
    }

    #[test]
    fn a_track_keeps_the_colour_it_is_given_and_it_is_one_undo_step() {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let was = session.project().track(track).unwrap().color;
        let wanted = Color::PALETTE
            .iter()
            .copied()
            .find(|color| *color != was)
            .expect("the palette has more than one entry");
        session.forget_history();

        session.set_track_color(track, wanted).unwrap();
        assert_eq!(session.project().track(track).unwrap().color, wanted);
        // The same colour again is not an edit; a palette full of them would otherwise fill the
        // undo stack with steps that undo nothing visible.
        session.set_track_color(track, wanted).unwrap();
        assert_eq!(undo_depth(&mut session), 1);

        session.set_track_color(track, was).unwrap();
        assert_eq!(session.undo(), Some(Edit::SetTrackColor));
        assert_eq!(session.project().track(track).unwrap().color, wanted);
    }

    #[test]
    fn resizing_a_lane_is_an_edit_like_every_other_track_property() {
        // It writes a stored field, so it has to go through `record` — which is what puts the
        // step on the undo stack and, just as importantly, what marks the document dirty. Written
        // straight, a resize was neither undoable nor autosaved: the lane came back its old height
        // on the next open unless something else happened to be edited after it.
        let mut session = session();
        let track = session.add_audio_track("Vocals");
        session.forget_history();
        assert!(!session.is_dirty());
        let was = session.project().track(track).unwrap().height;

        session.set_track_height(track, 180.0).unwrap();
        assert_eq!(session.project().track(track).unwrap().height, 180.0);
        assert!(session.is_dirty(), "a resize is a change to the document");

        session.undo().expect("a step to undo");
        assert_eq!(session.project().track(track).unwrap().height, was);

        // Out of range and not-a-number both land inside the bounds rather than in the document.
        session.set_track_height(track, 10_000.0).unwrap();
        assert_eq!(
            session.project().track(track).unwrap().height,
            MAX_TRACK_HEIGHT
        );
        session.set_track_height(track, f32::NAN).unwrap();
        assert_eq!(
            session.project().track(track).unwrap().height,
            MIN_TRACK_HEIGHT
        );

        // And setting the height it already has is not a step, like every sibling command.
        let depth = undo_depth(&mut session);
        session.set_track_height(track, MIN_TRACK_HEIGHT).unwrap();
        assert_eq!(undo_depth(&mut session), depth);
    }
}
