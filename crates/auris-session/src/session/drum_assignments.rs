//! Manual musical assignments for drum tracks, independent of acoustic measurements.

use auris_core::TrackId;
use auris_core::project::{DrumLane, DrumMap, DrumRole};

use super::Session;
use crate::{DrumMapSoundFont, DrumMapSource, Edit, SessionError};

impl Session {
    /// The current musical assignments of a drum track, or `None` for another track kind.
    ///
    /// A drum track without an authored or accepted map returns an empty map. No role is inferred
    /// from a SoundFont bank, instrument name or note pitch. Newly created built-in kits already
    /// carry their authored assignments; an explicitly empty or partial map stays that way.
    pub fn drum_assignments(&self, track: TrackId) -> Option<DrumMap> {
        let track = self.project.track(track)?;
        if !track.kind.is_drum() {
            return None;
        }
        Some(DrumMap::load(&track.kind.as_instrument()?.instrument_state).unwrap_or_default())
    }

    /// The ordered manual-programming lanes of a drum track.
    pub fn drum_lanes(&self, track: TrackId) -> Option<Vec<DrumLane>> {
        self.drum_assignments(track).map(|map| map.lanes)
    }

    /// The MIDI key names exposed by the drum track's live CLAP instrument.
    ///
    /// An empty list means the source is built in, is not available, or does not implement the
    /// CLAP note-name extension. These names are presentation metadata: asking never edits the
    /// project or changes what the instrument plays.
    pub fn drum_plugin_note_names(&mut self, track: TrackId) -> Vec<(u8, String)> {
        self.hosted.instrument_note_names(track)
    }

    /// A source-authored starting map for a drum track that has no saved user map.
    ///
    /// CLAP note names are the most precise answer. The built-in kit carries its six authored
    /// generation roles, and a SoundFont percussion bank gets the standard General MIDI map.
    /// Unknown sources deliberately return an empty map instead of inheriting another sound's
    /// labels.
    pub fn suggested_drum_map(&mut self, track: TrackId) -> Option<DrumMap> {
        let source = self.drum_map_source(track)?;
        let note_names = self.drum_plugin_note_names(track);
        if !note_names.is_empty() {
            let mut map = DrumMap::default();
            for (note, name) in note_names {
                map.add_lane(note, name);
            }
            return Some(map);
        }
        if source.instrument_id == auris_synth::DrumKit::ID {
            return Some(DrumMap::from_voices([
                (DrumRole::Kick, 36),
                (DrumRole::Snare, 38),
                (DrumRole::ClosedHat, 42),
                (DrumRole::OpenHat, 46),
                (DrumRole::Crash, 49),
                (DrumRole::Tom, 47),
            ]));
        }
        if source
            .soundfont
            .as_ref()
            .is_some_and(|preset| preset.bank == 128)
        {
            return Some(DrumMap::general_midi());
        }
        Some(DrumMap::default())
    }

    /// Stable user-library key for the sound source selected on a drum track.
    pub fn drum_map_source(&self, track: TrackId) -> Option<DrumMapSource> {
        let entry = self.project.track(track)?;
        if !entry.kind.is_drum() {
            return None;
        }
        let instrument = entry.kind.as_instrument()?;
        let plugin_file = instrument
            .file
            .as_ref()
            .and_then(|path| path.resolve(self.project_folder()));
        let soundfont = (instrument.instrument_id == auris_sampler::SAMPLER_ID)
            .then(|| auris_sampler::stored_preset(&instrument.instrument_state))
            .flatten()
            .and_then(|preset| {
                let font = self.project.soundfonts.get(&preset.font)?;
                Some(DrumMapSoundFont {
                    path: font.path.resolve(self.project_folder())?,
                    byte_size: font.byte_size,
                    bank: preset.bank,
                    patch: preset.patch,
                })
            });
        Some(DrumMapSource {
            instrument_id: instrument.instrument_id.clone(),
            plugin_file,
            soundfont,
        })
    }

    /// Replaces a drum track's complete editor map without moving any existing hit.
    ///
    /// This is the command used by saved maps and templates. The map is embedded in the project,
    /// so opening the project never depends on a user-level library file being present.
    pub fn set_drum_map(&mut self, track: TrackId, map: DrumMap) -> Result<bool, SessionError> {
        self.replace_drum_map(track, map, true)
    }

    /// Installs the map selected for a newly chosen sound source without a second undo step.
    ///
    /// The caller must use this only immediately after an instrument-changing command has
    /// already recorded the complete pre-change project. The replacement then becomes part of
    /// that command's redo snapshot instead of looking like a separate mapping edit.
    pub fn set_drum_source_map(
        &mut self,
        track: TrackId,
        map: DrumMap,
    ) -> Result<bool, SessionError> {
        self.replace_drum_map(track, map, false)
    }

    fn replace_drum_map(
        &mut self,
        track: TrackId,
        map: DrumMap,
        record: bool,
    ) -> Result<bool, SessionError> {
        let current = self.checked_drum_map(track)?;
        let mut next_state = self
            .project
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state
            .clone();
        ensure_map_storage(&next_state)?;
        map.store(&mut next_state);
        let next = DrumMap::load(&next_state).unwrap_or_default();
        if current == next {
            return Ok(false);
        }
        if record {
            self.record(Edit::SetDrumAssignment);
        }
        self.project
            .track_mut(track)
            .unwrap()
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_state = next_state;
        Ok(true)
    }

    /// Appends a named manual lane and returns its stable identity.
    pub fn add_drum_lane(
        &mut self,
        track: TrackId,
        note: u8,
        name: impl Into<String>,
    ) -> Result<u64, SessionError> {
        validate_note(note)?;
        let mut map = self.checked_drum_map(track)?;
        if map.lanes.iter().any(|lane| lane.note == note) {
            return Err(SessionError::InvalidDrumAssignment(format!(
                "MIDI note {note} already has a drum lane"
            )));
        }
        let id = map.add_lane(note, name);
        self.set_drum_map(track, map)?;
        Ok(id)
    }

    /// Renames one manual drum lane. Empty restores its derived role or MIDI label.
    pub fn rename_drum_lane(
        &mut self,
        track: TrackId,
        lane: u64,
        name: impl Into<String>,
    ) -> Result<bool, SessionError> {
        let mut map = self.checked_drum_map(track)?;
        let lane = map
            .lanes
            .iter_mut()
            .find(|candidate| candidate.id == lane)
            .ok_or_else(|| invalid_lane(lane))?;
        let name = name.into().trim().to_string();
        if lane.name == name {
            return Ok(false);
        }
        lane.name = name;
        self.set_drum_map(track, map)
    }

    /// Changes the physical MIDI key of one lane.
    ///
    /// When `move_existing_hits` is true, every current clip hit on the old key moves with the
    /// lane. Otherwise only the mapping changes, leaving the score untouched.
    pub fn set_drum_lane_note(
        &mut self,
        track: TrackId,
        lane: u64,
        note: u8,
        move_existing_hits: bool,
    ) -> Result<bool, SessionError> {
        validate_note(note)?;
        let mut map = self.checked_drum_map(track)?;
        if map
            .lanes
            .iter()
            .any(|candidate| candidate.id != lane && candidate.note == note)
        {
            return Err(SessionError::InvalidDrumAssignment(format!(
                "MIDI note {note} already has a drum lane"
            )));
        }
        let target = map
            .lanes
            .iter_mut()
            .find(|candidate| candidate.id == lane)
            .ok_or_else(|| invalid_lane(lane))?;
        let old_note = target.note;
        if old_note == note {
            return Ok(false);
        }
        target.note = note;
        map.sync_voices_from_lanes();

        self.begin_transaction(Edit::SetDrumAssignment);
        let changed = self.set_drum_map(track, map)?;
        let mut moved = false;
        if move_existing_hits {
            let entry = self.project.track_mut(track).unwrap();
            for clip in entry.kind.note_clips_mut().unwrap() {
                for hit in &mut clip.notes {
                    if hit.pitch == old_note {
                        hit.pitch = note;
                        moved = true;
                    }
                }
            }
        }
        if moved {
            self.invalidate_graph();
        }
        self.end_transaction();
        Ok(changed)
    }

    /// Enables or disables one automatic-generation role on a manual lane.
    ///
    /// A role belongs to at most one lane. Enabling it here removes it from the previous lane.
    pub fn set_drum_lane_role(
        &mut self,
        track: TrackId,
        lane: u64,
        role: DrumRole,
        enabled: bool,
    ) -> Result<bool, SessionError> {
        let mut map = self.checked_drum_map(track)?;
        if !map.lanes.iter().any(|candidate| candidate.id == lane) {
            return Err(invalid_lane(lane));
        }
        let before = map.clone();
        if enabled {
            for candidate in &mut map.lanes {
                candidate.roles.remove(&role);
            }
            map.lanes
                .iter_mut()
                .find(|candidate| candidate.id == lane)
                .unwrap()
                .roles
                .insert(role);
        } else {
            map.lanes
                .iter_mut()
                .find(|candidate| candidate.id == lane)
                .unwrap()
                .roles
                .remove(&role);
        }
        map.sync_voices_from_lanes();
        if map == before {
            return Ok(false);
        }
        self.set_drum_map(track, map)
    }

    /// Removes a manual lane. Existing notes remain as unmapped MIDI hits.
    pub fn remove_drum_lane(&mut self, track: TrackId, lane: u64) -> Result<bool, SessionError> {
        let mut map = self.checked_drum_map(track)?;
        if !map.remove_lane(lane) {
            return Err(invalid_lane(lane));
        }
        self.set_drum_map(track, map)
    }

    /// Moves a manual lane in presentation order.
    pub fn move_drum_lane(
        &mut self,
        track: TrackId,
        lane: u64,
        offset: i32,
    ) -> Result<bool, SessionError> {
        let mut map = self.checked_drum_map(track)?;
        if !map.lanes.iter().any(|candidate| candidate.id == lane) {
            return Err(invalid_lane(lane));
        }
        if !map.move_lane(lane, offset) {
            return Ok(false);
        }
        self.set_drum_map(track, map)
    }

    fn checked_drum_map(&self, track: TrackId) -> Result<DrumMap, SessionError> {
        let entry = self
            .project
            .track(track)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        if !entry.kind.is_drum() {
            return Err(SessionError::WrongTrackKind {
                id: track.0,
                actual: entry.kind.label(),
                expected: "a drum track",
            });
        }
        ensure_map_storage(&entry.kind.as_instrument().unwrap().instrument_state)?;
        Ok(
            DrumMap::load(&entry.kind.as_instrument().unwrap().instrument_state)
                .unwrap_or_default(),
        )
    }

    /// Adds, changes or removes one musical role's MIDI address on a drum track.
    ///
    /// `Some(note)` assigns a key from 0 through 127; `None` removes the role. Other roles and
    /// the instrument's own saved state are preserved. An actual change is one undoable edit;
    /// an identical assignment or removal of a missing role changes nothing.
    ///
    /// The map supplies drum editor labels and future generation. This command never rewrites
    /// existing notes, recipes or performance transforms; existing recipes retain their own
    /// assignments for later regeneration. Removing the last role stores an explicit empty map.
    /// Musical assignments do not change the analysis source or rebuild the audio graph.
    pub fn set_drum_assignment(
        &mut self,
        track: TrackId,
        role: DrumRole,
        note: Option<u8>,
    ) -> Result<bool, SessionError> {
        let mut map = self.checked_drum_map(track)?;
        if let Some(note) = note {
            validate_note(note)?;
        }
        if map.voices.get(&role).copied() == note {
            return Ok(false);
        }
        match note {
            Some(note) => {
                for lane in &mut map.lanes {
                    lane.roles.remove(&role);
                }
                let id = map.add_lane(note, String::new());
                map.lanes
                    .iter_mut()
                    .find(|lane| lane.id == id)
                    .unwrap()
                    .roles
                    .insert(role);
            }
            None => {
                for lane in &mut map.lanes {
                    lane.roles.remove(&role);
                }
            }
        }
        map.sync_voices_from_lanes();
        self.set_drum_map(track, map)
    }
}

fn validate_note(note: u8) -> Result<(), SessionError> {
    if note > 127 {
        return Err(SessionError::InvalidDrumAssignment(
            "MIDI notes must be between 0 and 127".into(),
        ));
    }
    Ok(())
}

fn ensure_map_storage(state: &auris_core::PluginState) -> Result<(), SessionError> {
    if !state.extra.is_null() && !state.extra.is_object() {
        return Err(SessionError::InvalidDrumAssignment(
            "the instrument's extra state cannot hold an assignment without replacing its data"
                .into(),
        ));
    }
    Ok(())
}

fn invalid_lane(lane: u64) -> SessionError {
    SessionError::InvalidDrumAssignment(format!("unknown drum lane {lane}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{BAR, Scratch, named_font, session};
    use auris_core::{ClipPreset, ClipRecipe, PresetRef, Ticks};

    #[test]
    fn assignments_preserve_existing_takes_and_change_only_future_generation() {
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let authored = session.drum_assignments(track).unwrap();
        assert_eq!(authored.voices[&DrumRole::Snare], 38);
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR,
                ClipRecipe::new(ClipPreset::Snare, 7),
            )
            .unwrap();
        let original = session.midi_clip(clip).unwrap().clone();
        assert!(!original.notes.is_empty());
        let source = session.drum_analysis_source_key(track);
        session.forget_history();
        assert!(
            session
                .set_drum_assignment(track, DrumRole::Snare, Some(84))
                .unwrap()
        );
        let changed = session.drum_assignments(track).unwrap();
        assert_eq!(changed.voices[&DrumRole::Snare], 84);
        for (role, note) in &authored.voices {
            if *role != DrumRole::Snare {
                assert_eq!(changed.voices.get(role), Some(note));
            }
        }
        assert_eq!(session.midi_clip(clip).unwrap(), &original);
        assert_eq!(session.drum_analysis_source_key(track), source);
        assert_eq!(session.undo(), Some(Edit::SetDrumAssignment));
        assert_eq!(session.drum_assignments(track), Some(authored));
        assert_eq!(session.redo(), Some(Edit::SetDrumAssignment));
        assert_eq!(session.drum_assignments(track), Some(changed));
        session.regenerate_clip(clip).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap(), &original);
        let next = session
            .generate_clip(track, BAR, BAR, ClipRecipe::new(ClipPreset::Snare, 7))
            .unwrap();
        assert!(!session.midi_clip(next).unwrap().notes.is_empty());
        assert!(
            session
                .midi_clip(next)
                .unwrap()
                .notes
                .iter()
                .all(|note| note.pitch == 84)
        );
    }

    #[test]
    fn empty_soundfont_assignments_can_be_added_removed_and_saved_without_defaults() {
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let font = named_font(&mut session, "Custom kit");
        let preset = PresetRef {
            font,
            bank: 128,
            patch: 0,
        };
        session.set_track_preset(track, preset).unwrap();
        assert_eq!(session.drum_assignments(track), Some(DrumMap::default()));
        let unassigned = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR,
                ClipRecipe::new(ClipPreset::Drums, 1),
            )
            .unwrap();
        assert!(session.midi_clip(unassigned).unwrap().notes.is_empty());
        assert_eq!(
            session.clip_recipe(unassigned).unwrap().drum_map,
            Some(DrumMap::default())
        );
        assert!(
            session
                .set_drum_assignment(track, DrumRole::Kick, Some(0))
                .unwrap()
        );
        let assigned = session
            .generate_clip(track, BAR, BAR, ClipRecipe::new(ClipPreset::Drums, 1))
            .unwrap();
        let hits = &session.midi_clip(assigned).unwrap().notes;
        assert!(!hits.is_empty());
        assert!(
            hits.iter()
                .all(|note| note.pitch == 0 && note.drum_voice == "kick")
        );
        session.regenerate_clip(unassigned).unwrap();
        assert!(session.midi_clip(unassigned).unwrap().notes.is_empty());
        assert!(
            session
                .set_drum_assignment(track, DrumRole::Snare, Some(127))
                .unwrap()
        );
        assert_eq!(session.track_preset(track), Some(preset));
        assert!(
            session
                .set_drum_assignment(track, DrumRole::Kick, None)
                .unwrap()
        );
        assert!(
            session
                .set_drum_assignment(track, DrumRole::Snare, None)
                .unwrap()
        );
        let state = &session
            .project
            .track(track)
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .instrument_state;
        let manual = DrumMap::load(state).unwrap();
        assert!(manual.voices.is_empty());
        assert_eq!(
            manual
                .lanes
                .iter()
                .map(|lane| lane.note)
                .collect::<Vec<_>>(),
            [0, 127]
        );
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR,
                ClipRecipe::new(ClipPreset::Drums, 1),
            )
            .unwrap();
        assert!(session.midi_clip(clip).unwrap().notes.is_empty());
        let scratch = Scratch::new("manual-drum-map");
        let document = session
            .save_as(&scratch.join("Kit.auris"))
            .unwrap()
            .document;
        let mut loaded = crate::session::fixtures::session();
        loaded.open(&document).unwrap();
        assert_eq!(loaded.drum_assignments(track), Some(manual));
        assert_eq!(loaded.track_preset(track), Some(preset));
        assert!(loaded.midi_clip(clip).unwrap().notes.is_empty());
    }

    #[test]
    fn manual_lanes_are_named_ordered_role_optional_and_can_move_existing_hits() {
        let mut session = session();
        let track = session.add_default_drum_track("Kit").unwrap();
        let clip = session
            .add_midi_clip(track, "Beat", Ticks::ZERO, BAR)
            .unwrap();
        session
            .add_note(clip, auris_core::Note::new(39, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.forget_history();

        let lane = session.add_drum_lane(track, 39, "Clap").unwrap();
        assert!(session.rename_drum_lane(track, lane, "Hand Clap").unwrap());
        assert!(
            session
                .set_drum_lane_role(track, lane, DrumRole::Snare, true)
                .unwrap()
        );
        let map = session.drum_assignments(track).unwrap();
        assert_eq!(map.voices[&DrumRole::Snare], 39);
        assert_eq!(
            map.lanes.iter().find(|item| item.id == lane).unwrap().name,
            "Hand Clap"
        );
        assert!(session.move_drum_lane(track, lane, -20).unwrap());
        assert_eq!(session.drum_lanes(track).unwrap()[0].id, lane);

        assert!(session.set_drum_lane_note(track, lane, 40, false).unwrap());
        assert_eq!(session.midi_clip(clip).unwrap().notes[0].pitch, 39);
        assert!(session.set_drum_lane_note(track, lane, 41, true).unwrap());
        assert_eq!(session.midi_clip(clip).unwrap().notes[0].pitch, 39);
        session
            .add_note(
                clip,
                auris_core::Note::new(41, Ticks::QUARTER, Ticks::QUARTER),
            )
            .unwrap();
        assert!(session.set_drum_lane_note(track, lane, 42, true).is_err());
        assert!(session.set_drum_lane_note(track, lane, 43, true).unwrap());
        assert_eq!(session.midi_clip(clip).unwrap().notes[1].pitch, 43);

        let before_remove = session.drum_assignments(track).unwrap();
        assert!(session.remove_drum_lane(track, lane).unwrap());
        assert!(
            !session
                .drum_assignments(track)
                .unwrap()
                .lanes
                .iter()
                .any(|item| item.id == lane)
        );
        assert_eq!(session.undo(), Some(Edit::SetDrumAssignment));
        assert_eq!(session.drum_assignments(track), Some(before_remove));
    }

    #[test]
    fn invalid_or_unchanged_assignments_leave_history_and_source_state_alone() {
        let mut session = session();
        let drum = session.add_default_drum_track("Kit").unwrap();
        let melodic = session.add_default_instrument_track("Keys").unwrap();
        session.forget_history();
        assert_eq!(session.drum_assignments(melodic), None);
        assert!(
            session
                .set_drum_assignment(melodic, DrumRole::Kick, Some(36))
                .is_err()
        );
        assert!(
            session
                .set_drum_assignment(TrackId(u64::MAX), DrumRole::Kick, Some(36))
                .is_err()
        );
        for note in [128, 255] {
            assert!(
                session
                    .set_drum_assignment(drum, DrumRole::Kick, Some(note))
                    .is_err()
            );
        }
        assert!(
            !session
                .set_drum_assignment(drum, DrumRole::Kick, Some(36))
                .unwrap()
        );
        assert!(!session.can_undo());
        let state = &mut session
            .project
            .track_mut(drum)
            .unwrap()
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_state;
        state.set_hosted_bytes(&[9, 8, 7]);
        assert!(
            session
                .set_drum_assignment(drum, DrumRole::Kick, Some(73))
                .unwrap()
        );
        assert_eq!(
            session
                .project
                .track(drum)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap()
                .instrument_state
                .hosted_bytes(),
            Some(vec![9, 8, 7])
        );
        session.forget_history();
        let state = &mut session
            .project
            .track_mut(drum)
            .unwrap()
            .kind
            .as_instrument_mut()
            .unwrap()
            .instrument_state;
        state.extra = serde_json::json!(["opaque"]);
        assert!(
            session
                .set_drum_assignment(drum, DrumRole::Kick, Some(73))
                .is_err()
        );
        assert_eq!(
            session
                .project
                .track(drum)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap()
                .instrument_state
                .extra,
            serde_json::json!(["opaque"])
        );
        assert!(!session.can_undo());
    }
}
