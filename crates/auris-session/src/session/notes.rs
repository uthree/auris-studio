//! What is inside a MIDI clip.
//!
//! Adding, deleting, duplicating, transposing, moving and resizing notes, and how hard each is
//! struck. Every one of them takes a [`ClipId`] and an index or a slice of indices, because the
//! piano roll edits a selection and a selection is what a gesture produces.
//!
//! Two of them come in pairs — one note and many — and the many-note form is not a loop over the
//! single-note one: a chord played harder has to keep its shape, so the whole selection is scaled
//! by whatever headroom its loudest note has left.

use auris_core::time::Ticks;
use auris_core::{ClipId, MidiClip, Note};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

impl Session {
    /// Adds a note to a MIDI clip, returning its index.
    pub fn add_note(&mut self, clip: ClipId, note: Note) -> Result<usize, SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.record(Edit::AddNote);
        let grid = self.project.grid;
        let mut index = 0;
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.notes.push(Note {
                pitch: note.pitch.min(127),
                velocity: Self::finite_unit(note.velocity),
                start: note.start.max_zero(),
                length: Ticks(note.length.raw().max(1)),
            });
            target.fit_length_to_notes(grid);
            index = target.notes.len() - 1;
        }
        self.invalidate_graph();
        Ok(index)
    }

    /// Removes notes by index. Indices that do not exist are ignored.
    pub fn remove_notes(&mut self, clip: ClipId, indices: &[usize]) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        if indices.is_empty() {
            return Ok(());
        }
        self.record(Edit::DeleteNotes);
        let mut doomed: Vec<usize> = indices.to_vec();
        doomed.sort_unstable();
        doomed.dedup();
        if let Some(target) = self.project.midi_clip_mut(clip) {
            // Remove from the back so the earlier indices stay valid.
            for index in doomed.into_iter().rev() {
                if index < target.notes.len() {
                    target.notes.remove(index);
                }
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Copies notes, offset by the length of the selection, and returns the copies' indices.
    ///
    /// Offsetting by the whole selection rather than by one note's length is what makes
    /// repeated duplication chain a figure end to end instead of piling copies on top of it.
    pub fn duplicate_notes(
        &mut self,
        clip: ClipId,
        indices: &[usize],
    ) -> Result<Vec<usize>, SessionError> {
        let Some(target) = self.project.midi_clip(clip).map(|(_, clip)| clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let chosen: Vec<Note> = indices
            .iter()
            .filter_map(|index| target.notes.get(*index).copied())
            .collect();
        if chosen.is_empty() {
            return Ok(Vec::new());
        }
        let first = chosen
            .iter()
            .map(|note| note.start)
            .min()
            .unwrap_or_default();
        let last = chosen.iter().map(Note::end).max().unwrap_or_default();
        let offset = last - first;

        self.record(Edit::DuplicateNotes);
        let grid = self.project.grid;
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let base = target.notes.len();
        for note in chosen {
            target.notes.push(Note {
                start: note.start + offset,
                ..note
            });
        }
        target.fit_length_to_notes(grid);
        let copies = (base..target.notes.len()).collect();
        self.invalidate_graph();
        Ok(copies)
    }

    /// Sets how hard the named notes are struck, from 0 to 1.
    ///
    /// The piano roll has painted a velocity heat map since it was written, and there was no
    /// command that could change what it was showing: the one thing the colour said about a note
    /// was the one thing about it nobody could edit.
    pub fn set_note_velocity(
        &mut self,
        clip: ClipId,
        indices: &[usize],
        velocity: f32,
    ) -> Result<(), SessionError> {
        let changes: Vec<(usize, f32)> = indices.iter().map(|index| (*index, velocity)).collect();
        self.set_note_velocities(clip, &changes)
    }

    /// Sets how hard individual notes are struck, each to a value of its own from 0 to 1.
    ///
    /// The per-note form is what a *gesture* needs. Dragging the dynamics of a chord has to keep
    /// the differences between its notes — a phrase written soft-loud-soft is still soft-loud-soft
    /// once it is played harder — so every note in the selection lands somewhere different, and
    /// one value for all of them cannot say that.
    ///
    /// An index that names no note is skipped rather than refused: a selection is held by
    /// position, and half a chord going through is better than a whole gesture failing because
    /// one note was deleted underneath it.
    pub fn set_note_velocities(
        &mut self,
        clip: ClipId,
        changes: &[(usize, f32)],
    ) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        // Nothing to record for a set that changes nothing — a menu applied to a chord already at
        // that level should not push an undo step, and neither should a drag that has not yet
        // travelled far enough to move a note off the value it started on.
        let unchanged = self
            .project
            .midi_clip(clip)
            .map(|(_, target)| {
                changes.iter().all(|(index, velocity)| {
                    target
                        .notes
                        .get(*index)
                        .is_none_or(|note| note.velocity == velocity.clamp(0.0, 1.0))
                })
            })
            .unwrap_or(true);
        if unchanged {
            return Ok(());
        }

        self.record(Edit::SetNoteVelocity);
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        for (index, velocity) in changes {
            if let Some(note) = target.notes.get_mut(*index) {
                note.velocity = Self::finite_unit(*velocity);
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Shifts notes in pitch, keeping the intervals between them.
    ///
    /// The whole selection moves by the same amount or not at all: clamping each note to the
    /// MIDI range separately would silently flatten a chord into a cluster.
    pub fn transpose_notes(
        &mut self,
        clip: ClipId,
        indices: &[usize],
        semitones: i32,
    ) -> Result<(), SessionError> {
        let Some(target) = self.project.midi_clip(clip).map(|(_, clip)| clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let pitches: Vec<u8> = indices
            .iter()
            .filter_map(|index| target.notes.get(*index).map(|note| note.pitch))
            .collect();
        let (Some(lowest), Some(highest)) =
            (pitches.iter().min().copied(), pitches.iter().max().copied())
        else {
            return Ok(());
        };
        let shift = semitones.max(-(lowest as i32)).min(127 - highest as i32);
        if shift == 0 {
            return Ok(());
        }

        self.record(Edit::TransposeNotes);
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        for index in indices {
            if let Some(note) = target.notes.get_mut(*index) {
                note.pitch = (note.pitch as i32 + shift).clamp(0, 127) as u8;
            }
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Moves notes by a tick and pitch delta, from positions captured before the gesture began.
    pub fn move_notes(
        &mut self,
        clip: ClipId,
        origins: &[(usize, Ticks, u8)],
        delta_ticks: Ticks,
        delta_pitch: i32,
    ) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.record(Edit::MoveNotes);
        let grid = self.project.grid;
        if let Some(target) = self.project.midi_clip_mut(clip) {
            for (index, start, pitch) in origins {
                if let Some(note) = target.notes.get_mut(*index) {
                    note.start = (*start + delta_ticks).max_zero();
                    note.pitch = (*pitch as i32 + delta_pitch).clamp(0, 127) as u8;
                }
            }
            target.fit_length_to_notes(grid);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Drags one note's end to `end`, clip-relative.
    pub fn resize_note(
        &mut self,
        clip: ClipId,
        index: usize,
        end: Ticks,
    ) -> Result<(), SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        self.record(Edit::ResizeNote);
        let grid = Ticks(self.project.grid.raw().max(1));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            if let Some(note) = target.notes.get_mut(index) {
                note.length = (end - note.start).max(grid);
            }
            target.fit_length_to_notes(grid);
        }
        self.invalidate_graph();
        Ok(())
    }

    /// A MIDI clip anywhere in the project.
    pub fn midi_clip(&self, clip: ClipId) -> Option<&MidiClip> {
        self.project.midi_clip(clip).map(|(_, clip)| clip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{session, session_with_clip, undo_depth};

    #[test]
    fn how_hard_a_note_is_struck_can_be_changed() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let first = session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let second = session
            .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();

        session
            .set_note_velocity(clip, &[first, second], 0.25)
            .unwrap();
        let velocities: Vec<f32> = session
            .midi_clip(clip)
            .unwrap()
            .notes
            .iter()
            .map(|note| note.velocity)
            .collect();
        assert_eq!(velocities, vec![0.25, 0.25]);

        assert_eq!(session.undo(), Some(Edit::SetNoteVelocity));
        assert!(session.midi_clip(clip).unwrap().notes[0].velocity > 0.25);

        // Out of range is clamped rather than refused, and a set that changes nothing is not an
        // edit — applying a marking to a chord already at it should not push an undo step.
        session.redo();
        session.set_note_velocity(clip, &[first], 4.0).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes[0].velocity, 1.0);
        let depth = undo_depth(&mut session);
        session.set_note_velocity(clip, &[first], 1.0).unwrap();
        assert_eq!(undo_depth(&mut session), depth);
    }

    #[test]
    fn a_chord_can_be_played_harder_without_losing_its_shape() {
        // What a velocity drag over a selection needs: each note goes somewhere of its own, so
        // the phrasing written into the part survives being made louder.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let quiet = session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let loud = session
            .add_note(clip, Note::new(64, Ticks::QUARTER, Ticks::QUARTER))
            .unwrap();

        session
            .set_note_velocities(clip, &[(quiet, 0.4), (loud, 0.6)])
            .unwrap();
        let velocities = |session: &Session| -> Vec<f32> {
            session
                .midi_clip(clip)
                .unwrap()
                .notes
                .iter()
                .map(|note| note.velocity)
                .collect()
        };
        assert_eq!(velocities(&session), vec![0.4, 0.6]);

        // One undo step for the pair, not one each: the whole gesture is a single edit.
        let depth = undo_depth(&mut session);
        session
            .set_note_velocities(clip, &[(quiet, 0.5), (loud, 0.7)])
            .unwrap();
        assert_eq!(velocities(&session), vec![0.5, 0.7]);
        assert_eq!(undo_depth(&mut session), depth + 1);
        assert_eq!(session.undo(), Some(Edit::SetNoteVelocity));
        assert_eq!(velocities(&session), vec![0.4, 0.6]);

        // A note that has gone is skipped, and the rest of the gesture still lands. A selection
        // is held by position, so one missing index must not throw the others away.
        session.remove_notes(clip, &[loud]).unwrap();
        session
            .set_note_velocities(clip, &[(quiet, 0.9), (loud, 0.9)])
            .unwrap();
        assert_eq!(velocities(&session), vec![0.9]);

        assert!(matches!(
            session.set_note_velocities(ClipId(9999), &[(0, 0.5)]),
            Err(SessionError::UnknownClip(_))
        ));
    }

    #[test]
    fn notes_are_clamped_into_range() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();

        let index = session
            .add_note(
                clip,
                Note {
                    pitch: 200,
                    velocity: 5.0,
                    start: Ticks(-500),
                    length: Ticks(0),
                },
            )
            .unwrap();
        let note = session.midi_clip(clip).unwrap().notes[index];
        assert_eq!(note.pitch, 127);
        assert_eq!(note.velocity, 1.0);
        assert_eq!(note.start, Ticks::ZERO);
        assert!(note.length.raw() >= 1);
    }

    #[test]
    fn removing_notes_takes_the_ones_asked_for() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        for pitch in [60, 62, 64, 65] {
            session
                .add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        }

        // Out-of-range and duplicate indices must not disturb the rest.
        session.remove_notes(clip, &[0, 2, 2, 99]).unwrap();
        let pitches: Vec<u8> = session
            .midi_clip(clip)
            .unwrap()
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        assert_eq!(pitches, vec![62, 65]);
    }

    #[test]
    fn duplicated_notes_chain_after_the_selection() {
        let (mut session, _, clip) = session_with_clip();
        let copies = session.duplicate_notes(clip, &[0, 1]).unwrap();
        assert_eq!(copies, vec![2, 3]);

        let notes = &session.midi_clip(clip).unwrap().notes;
        // The selection spans two quarters, so the copies start one half-note along.
        assert_eq!(notes[2].start, Ticks::from_beats(2.0));
        assert_eq!(notes[2].pitch, 60);
        assert_eq!(notes[3].start, Ticks::from_beats(3.0));
        assert_eq!(notes[3].pitch, 64);
    }

    #[test]
    fn transposing_keeps_the_intervals_when_it_would_run_off_the_end() {
        let (mut session, _, clip) = session_with_clip();
        session
            .add_note(clip, Note::new(120, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();

        // +12 would push 120 past 127, so the whole selection moves by 7 instead.
        session.transpose_notes(clip, &[0, 1, 2], 12).unwrap();
        let notes = &session.midi_clip(clip).unwrap().notes;
        assert_eq!(notes[0].pitch, 67);
        assert_eq!(notes[1].pitch, 71);
        assert_eq!(notes[2].pitch, 127);
        assert_eq!(
            notes[1].pitch - notes[0].pitch,
            4,
            "a clamped transposition must not flatten the interval"
        );
    }
}
