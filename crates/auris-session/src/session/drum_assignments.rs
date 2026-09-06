//! Manual musical assignments for drum tracks, independent of acoustic measurements.

use auris_core::TrackId;
use auris_core::project::{DrumMap, DrumRole};

use super::Session;
use crate::{Edit, SessionError};

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
        if note.is_some_and(|note| note > 127) {
            return Err(SessionError::InvalidDrumAssignment(
                "MIDI notes must be between 0 and 127".into(),
            ));
        }
        let state = &entry.kind.as_instrument().unwrap().instrument_state;
        let mut map = DrumMap::load(state).unwrap_or_default();
        if map.voices.get(&role).copied() == note {
            return Ok(false);
        }
        if !state.extra.is_null() && !state.extra.is_object() {
            return Err(SessionError::InvalidDrumAssignment(
                "the instrument's extra state cannot hold an assignment without replacing its data"
                    .into(),
            ));
        }
        match note {
            Some(note) => {
                map.voices.insert(role, note);
            }
            None => {
                map.voices.remove(&role);
            }
        }
        self.record(Edit::SetDrumAssignment);
        map.store(
            &mut self
                .project
                .track_mut(track)
                .unwrap()
                .kind
                .as_instrument_mut()
                .unwrap()
                .instrument_state,
        );
        Ok(true)
    }
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
        assert_eq!(DrumMap::load(state), Some(DrumMap::default()));
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
        assert_eq!(loaded.drum_assignments(track), Some(DrumMap::default()));
        assert_eq!(loaded.track_preset(track), Some(preset));
        assert!(loaded.midi_clip(clip).unwrap().notes.is_empty());
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
