//! Writing notes one step at a time from a keyboard, with the transport standing still.
//!
//! The other way to play a part in: no tempo to keep up with, no take to quantise afterwards.
//! A key goes down and the note sounds; when every key is up again, whatever was held is
//! written at the step cursor, one grid division long, and the cursor moves on a division.
//! A chord is therefore *delimited by release* — the notes of one are all the keys that were
//! down together — which is what lets a hand roll a chord on rather than strike it in one
//! instant, and is the rule that was settled for this when MIDI input was first planned.
//!
//! Both keyboards feed it, the hardware one and the typing one, through the same two calls a
//! note take listens on. The cursor is the playhead: each step seeks the transport to where the
//! next one will go, so the piano roll shows it without drawing anything new, and moving the
//! playhead by hand — the arrow keys, a click on the ruler — moves the cursor with it.
//!
//! Each chord is one undo step. A clip is made on the first one when step input was started on
//! a track with no clip to write into, and that is part of the same step.

use auris_core::{ClipId, Note, Ticks, TrackId};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

/// Step input under way.
#[derive(Clone, Debug)]
pub(super) struct StepInput {
    /// The track whose keys are written down.
    track: TrackId,
    /// The clip they are written into, once there is one.
    clip: Option<ClipId>,
    /// Where the next chord goes, on the timeline.
    cursor: Ticks,
    /// The chord being held: pitch and velocity, in the order struck.
    chord: Vec<(u8, f32)>,
    /// The keys still down.
    held: Vec<u8>,
}

/// What a frontend shows of step input.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StepInputState {
    /// The track being written onto.
    pub track: TrackId,
    /// The clip being written into, if one has been made yet.
    pub clip: Option<ClipId>,
    /// Where the next chord will go.
    pub cursor: Ticks,
    /// How long each step is: the editing grid.
    pub step: Ticks,
}

impl Session {
    /// Starts writing what is played on `track` one step at a time, into `clip` if given.
    ///
    /// The cursor starts at the playhead, on the grid, and never before the clip. With no clip,
    /// the first chord makes one where it lands.
    pub fn start_step_input(
        &mut self,
        track: TrackId,
        clip: Option<ClipId>,
    ) -> Result<(), SessionError> {
        let index = self.require_track(track)?;
        if !self.project.tracks[index].kind.holds_notes() {
            return Err(SessionError::NothingToRecordOnto);
        }
        let clip = match clip {
            Some(clip) => {
                let (owner, found) = self
                    .project
                    .midi_clip(clip)
                    .ok_or(SessionError::UnknownClip(clip.0))?;
                if owner != track {
                    return Err(SessionError::UnknownClip(clip.0));
                }
                Some((clip, found.start))
            }
            None => None,
        };
        let mut cursor = self.snap(self.playhead());
        if let Some((_, start)) = clip {
            cursor = cursor.max(start);
        }
        self.step_input = Some(StepInput {
            track,
            clip: clip.map(|(clip, _)| clip),
            cursor,
            chord: Vec::new(),
            held: Vec::new(),
        });
        self.follow_step_cursor();
        Ok(())
    }

    /// Stops step input. A chord still held is dropped: it was never let go of.
    pub fn stop_step_input(&mut self) {
        self.step_input = None;
    }

    /// Whether step input is on, and where.
    pub fn step_input(&self) -> Option<StepInputState> {
        self.step_input.as_ref().map(|step| StepInputState {
            track: step.track,
            clip: step.clip,
            cursor: step.cursor,
            step: self.step_length(),
        })
    }

    /// Moves the cursor on a step without writing anything: a rest.
    pub fn step_rest(&mut self) {
        let length = self.step_length();
        if let Some(step) = self.step_input.as_mut() {
            step.cursor = Ticks(step.cursor.raw() + length.raw());
        }
        self.follow_step_cursor();
    }

    /// Moves the cursor back a step and takes away what starts there in the clip, as one undo
    /// step. The backspace of step input: a wrong chord is undone and written again.
    pub fn step_back(&mut self) {
        let length = self.step_length();
        let Some(step) = self.step_input.as_mut() else {
            return;
        };
        let cursor = Ticks((step.cursor.raw() - length.raw()).max(0));
        step.cursor = cursor;
        step.chord.clear();
        let clip = step.clip;
        if let Some(clip) = clip
            && let Some((_, found)) = self.project.midi_clip(clip)
        {
            let at = Ticks(cursor.raw() - found.start.raw());
            let doomed: Vec<usize> = found
                .notes
                .iter()
                .enumerate()
                .filter(|(_, note)| note.start == at)
                .map(|(index, _)| index)
                .collect();
            if !doomed.is_empty() {
                let _ = self.remove_notes(clip, &doomed);
            }
        }
        self.follow_step_cursor();
    }

    /// How long one step is.
    fn step_length(&self) -> Ticks {
        Ticks(self.project.grid.raw().max(1))
    }

    /// Puts the playhead on the cursor, which is how the cursor is seen.
    fn follow_step_cursor(&mut self) {
        if let Some(cursor) = self.step_input.as_ref().map(|step| step.cursor)
            && !self.is_playing()
        {
            self.seek(cursor);
        }
    }

    /// Takes the cursor from the playhead, where somebody has moved it by hand.
    ///
    /// Only with an output running: a silent engine's playhead never moves, and reading it
    /// would send every chord back to the start. Moved by less than half a step counts as not
    /// moved, so a seek still on its way to the audio thread is not mistaken for a hand on it.
    fn adopt_moved_playhead(&mut self) {
        if !self.engine.is_running() || self.is_playing() {
            return;
        }
        let here = self.playhead();
        let half = self.step_length().raw() / 2;
        let snapped = self.snap(here);
        if let Some(step) = self.step_input.as_mut()
            && (here.raw() - step.cursor.raw()).abs() > half
        {
            step.cursor = snapped;
        }
    }

    /// A key went down on `track`.
    pub(super) fn step_struck(&mut self, track: TrackId, pitch: u8, velocity: f32) {
        if !self
            .step_input
            .as_ref()
            .is_some_and(|step| step.track == track)
        {
            return;
        }
        let starting = self
            .step_input
            .as_ref()
            .is_some_and(|step| step.held.is_empty() && step.chord.is_empty());
        if starting {
            self.adopt_moved_playhead();
        }
        let step = self.step_input.as_mut().expect("checked above");
        if !step.held.contains(&pitch) {
            step.held.push(pitch);
        }
        if !step.chord.iter().any(|held| held.0 == pitch) {
            step.chord.push((pitch, velocity));
        }
    }

    /// A key came up on `track`. The last one up writes the chord.
    pub(super) fn step_released(&mut self, track: TrackId, pitch: u8) {
        let Some(step) = self.step_input.as_mut().filter(|step| step.track == track) else {
            return;
        };
        step.held.retain(|held| *held != pitch);
        if !step.held.is_empty() || step.chord.is_empty() {
            return;
        }
        let chord = std::mem::take(&mut step.chord);
        let (track, clip, cursor) = (step.track, step.clip, step.cursor);
        match self.write_step(track, clip, cursor, &chord) {
            Ok(clip) => {
                let length = self.step_length();
                if let Some(step) = self.step_input.as_mut() {
                    step.clip = Some(clip);
                    step.cursor = Ticks(cursor.raw() + length.raw());
                }
                self.follow_step_cursor();
            }
            Err(error) => log::warn!("step input could not write a chord: {error}"),
        }
    }

    /// Writes one chord at `cursor`, making the clip if there is none, as one undo step.
    fn write_step(
        &mut self,
        track: TrackId,
        clip: Option<ClipId>,
        cursor: Ticks,
        chord: &[(u8, f32)],
    ) -> Result<ClipId, SessionError> {
        let length = self.step_length();
        self.begin_transaction(Edit::AddNote);
        let clip = match clip.filter(|clip| self.project.midi_clip(*clip).is_some()) {
            Some(clip) => clip,
            None => {
                let name = self
                    .project
                    .track(track)
                    .map(|found| found.name.clone())
                    .unwrap_or_default();
                match self.add_midi_clip(track, name, cursor, length) {
                    Ok(clip) => clip,
                    Err(error) => {
                        self.revert_transaction();
                        return Err(error);
                    }
                }
            }
        };
        let Some((_, found)) = self.project.midi_clip(clip) else {
            self.revert_transaction();
            return Err(SessionError::UnknownClip(clip.0));
        };
        let mut candidate = found.clone();
        let at = Ticks((cursor.raw() - candidate.start.raw()).max(0));
        for &(pitch, velocity) in chord {
            let mut note = Note::new(pitch.min(127), at, length);
            note.velocity = velocity.clamp(0.0, 1.0);
            candidate.notes.push(note);
        }
        let end = Ticks(at.raw() + length.raw());
        if end > candidate.length {
            candidate.length = end;
        }
        if let Err(error) = candidate.looped_note_instances() {
            self.revert_transaction();
            return Err(error.into());
        }
        if let Some(target) = self.project.midi_clip_mut(clip) {
            *target = candidate;
        }
        self.invalidate_graph();
        self.end_transaction();
        Ok(clip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::session;

    fn chord(session: &mut Session, track: TrackId, pitches: &[u8]) {
        for pitch in pitches {
            session.step_struck(track, *pitch, 0.8);
        }
        for pitch in pitches {
            session.step_released(track, *pitch);
        }
    }

    fn written(session: &Session, clip: ClipId) -> Vec<(i64, u8, i64)> {
        let (_, found) = session.project().midi_clip(clip).unwrap();
        let mut notes: Vec<_> = found
            .notes
            .iter()
            .map(|note| (note.start.raw(), note.pitch, note.length.raw()))
            .collect();
        notes.sort();
        notes
    }

    #[test]
    fn each_release_writes_a_step_and_moves_the_cursor_on() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.start_step_input(lead, None).unwrap();
        let grid = session.step_input().unwrap().step.raw();
        chord(&mut session, lead, &[60]);
        chord(&mut session, lead, &[62]);
        let clip = session.step_input().unwrap().clip.expect("a clip was made");
        assert_eq!(
            written(&session, clip),
            vec![(0, 60, grid), (grid, 62, grid)]
        );
        assert_eq!(session.step_input().unwrap().cursor, Ticks(2 * grid));
    }

    #[test]
    fn a_chord_is_every_key_down_together_however_it_was_rolled_on() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.start_step_input(lead, None).unwrap();
        session.step_struck(lead, 60, 0.8);
        session.step_struck(lead, 64, 0.8);
        session.step_released(lead, 60);
        // Still one key down: the chord is not over, and a key added now belongs to it.
        session.step_struck(lead, 67, 0.8);
        session.step_released(lead, 64);
        session.step_released(lead, 67);
        let clip = session.step_input().unwrap().clip.unwrap();
        let pitches: Vec<u8> = written(&session, clip).iter().map(|n| n.1).collect();
        assert_eq!(pitches, vec![60, 64, 67]);
        assert!(written(&session, clip).iter().all(|note| note.0 == 0));
    }

    #[test]
    fn a_rest_skips_a_step_and_back_takes_the_last_chord_away() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.start_step_input(lead, None).unwrap();
        let grid = session.step_input().unwrap().step.raw();
        chord(&mut session, lead, &[60]);
        session.step_rest();
        chord(&mut session, lead, &[64, 67]);
        let clip = session.step_input().unwrap().clip.unwrap();
        assert_eq!(
            written(&session, clip),
            vec![(0, 60, grid), (2 * grid, 64, grid), (2 * grid, 67, grid)]
        );
        session.step_back();
        assert_eq!(written(&session, clip), vec![(0, 60, grid)]);
        assert_eq!(session.step_input().unwrap().cursor, Ticks(2 * grid));
    }

    #[test]
    fn each_chord_is_one_undo_step_and_the_first_one_takes_its_clip_with_it() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        session.forget_history();
        session.start_step_input(lead, None).unwrap();
        chord(&mut session, lead, &[60, 64]);
        chord(&mut session, lead, &[62]);
        let clip = session.step_input().unwrap().clip.unwrap();
        assert!(session.undo().is_some());
        assert_eq!(written(&session, clip).len(), 2, "the second chord went");
        assert!(session.undo().is_some());
        assert!(
            session.project().midi_clip(clip).is_none(),
            "and the clip with the first"
        );
    }

    #[test]
    fn keys_on_another_track_are_not_written() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        let pad = session.add_default_instrument_track("Pad").unwrap();
        session.start_step_input(lead, None).unwrap();
        chord(&mut session, pad, &[60]);
        assert_eq!(session.step_input().unwrap().clip, None);
    }

    #[test]
    fn a_clip_given_is_written_into_from_its_own_start() {
        let mut session = session();
        let lead = session.add_default_instrument_track("Lead").unwrap();
        let grid = session.project().grid.raw();
        let clip = session
            .add_midi_clip(lead, "Verse", Ticks(4 * grid), Ticks(4 * grid))
            .unwrap();
        session.start_step_input(lead, Some(clip)).unwrap();
        assert_eq!(session.step_input().unwrap().cursor, Ticks(4 * grid));
        chord(&mut session, lead, &[72]);
        assert_eq!(written(&session, clip), vec![(0, 72, grid)]);
    }
}
