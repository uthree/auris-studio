//! Cut, copy and paste, for notes and for clips.
//!
//! The clipboard belongs to the session rather than to a frontend, for the reason every command
//! does: a second frontend should get it without writing it again. It is *not* the operating
//! system's clipboard — nothing here can be pasted into another application, and a project name
//! copied out of a text field cannot arrive here as eleven notes.
//!
//! # What is stored
//!
//! Positions, made relative to the copied material itself. Notes are stored with their starts
//! measured from the earliest of them, and clips with their starts measured from the earliest and
//! their tracks measured from the topmost. So the clipboard says *what the material looks like*
//! rather than where it used to be, and pasting it needs one position and nothing else.
//!
//! That is also what makes a paste survive the project changing underneath it. A clip block copied
//! off tracks four to seven still lands on four consecutive tracks after track two has been
//! deleted, because it was never holding those tracks' ids.
//!
//! # What a paste refuses
//!
//! Nothing, out loud. A copied MIDI clip aimed at an audio track is skipped, as is one aimed past
//! the bottom of the track list, and an audio clip whose source has since been removed. The rest
//! of the paste lands, and the count that comes back is what actually arrived — the same bargain
//! [`Session::remove_clips`](crate::Session::remove_clips) makes with ids that no longer exist,
//! and for the same reason: half a paste beats a whole gesture failing because one of six clips
//! had nowhere to go.

use auris_core::time::Ticks;
use auris_core::{AudioClip, ClipId, MidiClip, Note, TrackId, TrackKind};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

/// What was last copied.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Clipboard {
    /// Nothing has been copied yet.
    #[default]
    Empty,
    /// Notes out of one clip, their starts measured from the earliest of them.
    Notes(Vec<Note>),
    /// Clips off the timeline.
    Clips(Vec<CopiedClip>),
}

impl Clipboard {
    /// `true` when there is nothing to paste.
    pub fn is_empty(&self) -> bool {
        match self {
            Clipboard::Empty => true,
            Clipboard::Notes(notes) => notes.is_empty(),
            Clipboard::Clips(clips) => clips.is_empty(),
        }
    }

    /// How many things are on it.
    pub fn len(&self) -> usize {
        match self {
            Clipboard::Empty => 0,
            Clipboard::Notes(notes) => notes.len(),
            Clipboard::Clips(clips) => clips.len(),
        }
    }
}

/// One clip on the clipboard: where it sat relative to the rest, and what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct CopiedClip {
    /// How far after the earliest copied clip this one starts.
    pub offset: Ticks,
    /// How many tracks below the topmost copied clip's track this one sat.
    ///
    /// A row distance rather than a track id, so a block copied off four tracks lands on four
    /// consecutive tracks wherever it is pasted — and goes on doing so after the tracks it came
    /// from have been reordered, renamed or deleted.
    pub lane: usize,
    /// The clip itself, still carrying the id it was copied from. A paste hands out a fresh one.
    pub content: CopiedContent,
}

/// A copied clip's contents, which decide what kind of track it can land on.
#[derive(Clone, Debug, PartialEq)]
pub enum CopiedContent {
    /// Notes, and everything else a MIDI clip carries — including its recipe, so a pasted
    /// generated clip is still a generated clip.
    Midi(Box<MidiClip>),
    /// A reference to imported audio, with its trim and its fades.
    Audio(Box<AudioClip>),
}

impl Session {
    /// What is on the clipboard.
    pub fn clipboard(&self) -> &Clipboard {
        &self.clipboard
    }

    /// Puts notes of a clip on the clipboard, and says how many.
    ///
    /// Not an undo step and not a change: copying reads the document and writes nothing. Indices
    /// that name no note are skipped, because a selection is held by position and one deleted note
    /// should not empty the clipboard.
    ///
    /// A copy of nothing leaves whatever was already there. Emptying the clipboard on a stray
    /// ⌘C over an empty selection is a way to lose material a user believed they still had.
    pub fn copy_notes(&mut self, clip: ClipId, indices: &[usize]) -> usize {
        let Some((_, midi)) = self.project.midi_clip(clip) else {
            return 0;
        };
        let mut chosen: Vec<Note> = indices
            .iter()
            .filter_map(|index| midi.notes.get(*index).copied())
            .collect();
        let Some(first) = chosen.iter().map(|note| note.start).min() else {
            return 0;
        };
        for note in &mut chosen {
            note.start -= first;
        }
        let count = chosen.len();
        self.clipboard = Clipboard::Notes(chosen);
        count
    }

    /// Copies notes and then removes them.
    pub fn cut_notes(&mut self, clip: ClipId, indices: &[usize]) -> Result<usize, SessionError> {
        let count = self.copy_notes(clip, indices);
        if count == 0 {
            // Nothing was taken, so nothing is removed — and no undo step is pushed for a
            // gesture that did nothing.
            return Ok(0);
        }
        self.remove_notes_as(Edit::CutNotes, clip, indices)?;
        Ok(count)
    }

    /// Lays the clipboard's notes into a clip, starting at `at`, and returns their indices.
    ///
    /// `at` is clip-relative, like every other note position. It is clamped to the clip's start
    /// rather than refused: a playhead parked before the clip is a paste aimed at the beginning of
    /// it, which is what somebody meant.
    ///
    /// The clip grows to hold what arrives, exactly as adding a note by hand grows it.
    pub fn paste_notes(&mut self, clip: ClipId, at: Ticks) -> Result<Vec<usize>, SessionError> {
        if self.project.midi_clip(clip).is_none() {
            return Err(SessionError::UnknownClip(clip.0));
        }
        let Clipboard::Notes(notes) = &self.clipboard else {
            return Ok(Vec::new());
        };
        if notes.is_empty() {
            return Ok(Vec::new());
        }
        let notes = notes.clone();
        let at = at.max_zero();

        self.record(Edit::PasteNotes);
        let grid = self.project.grid;
        let Some(target) = self.project.midi_clip_mut(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        let base = target.notes.len();
        for note in notes {
            target.notes.push(Note {
                start: at + note.start,
                ..note
            });
        }
        target.fit_length_to_notes(grid);
        let pasted = (base..target.notes.len()).collect();
        self.invalidate_graph();
        Ok(pasted)
    }

    /// Puts clips on the clipboard, and says how many.
    ///
    /// Ids that name no clip are skipped, for the reason [`Self::copy_notes`] skips indices that
    /// name no note. A copy that finds nothing leaves the clipboard alone.
    pub fn copy_clips(&mut self, clips: &[ClipId]) -> usize {
        let mut found: Vec<(usize, Ticks, CopiedContent)> = Vec::new();
        for (row, track) in self.project.tracks.iter().enumerate() {
            match &track.kind {
                TrackKind::Instrument(inner) => {
                    for midi in inner.clips.iter().filter(|clip| clips.contains(&clip.id)) {
                        found.push((row, midi.start, CopiedContent::Midi(Box::new(midi.clone()))));
                    }
                }
                TrackKind::Audio(inner) => {
                    for audio in inner.clips.iter().filter(|clip| clips.contains(&clip.id)) {
                        found.push((
                            row,
                            audio.start,
                            CopiedContent::Audio(Box::new(audio.clone())),
                        ));
                    }
                }
                TrackKind::Bus => {}
            }
        }
        let (Some(first), Some(top)) = (
            found.iter().map(|(_, start, _)| *start).min(),
            found.iter().map(|(row, _, _)| *row).min(),
        ) else {
            return 0;
        };

        let copied: Vec<CopiedClip> = found
            .into_iter()
            .map(|(row, start, content)| CopiedClip {
                offset: start - first,
                lane: row - top,
                content,
            })
            .collect();
        let count = copied.len();
        self.clipboard = Clipboard::Clips(copied);
        count
    }

    /// Copies clips and then removes them.
    pub fn cut_clips(&mut self, clips: &[ClipId]) -> Result<usize, SessionError> {
        let count = self.copy_clips(clips);
        if count == 0 {
            return Ok(0);
        }
        self.remove_clips_as(Edit::CutClips, clips)?;
        Ok(count)
    }

    /// Lays the clipboard's clips down from `track` and `at`, and returns the new clips' ids.
    ///
    /// `track` is where the topmost copied clip lands; everything below it lands that many rows
    /// further down. A clip with nowhere to go — past the bottom of the list, on a track of the
    /// wrong kind, or naming audio that has since been removed — is skipped, and the rest arrive.
    pub fn paste_clips(&mut self, track: TrackId, at: Ticks) -> Result<Vec<ClipId>, SessionError> {
        let base = self
            .project
            .track_index(track)
            .ok_or(SessionError::UnknownTrack(track.0))?;
        let Clipboard::Clips(clips) = &self.clipboard else {
            return Ok(Vec::new());
        };
        if clips.is_empty() {
            return Ok(Vec::new());
        }
        let clips = clips.clone();
        let at = self.snap(at).max_zero();

        // Validated before anything is recorded, so a paste with nowhere at all to land leaves no
        // phantom undo step behind — the rule every refusing command in this crate keeps.
        let landing: Vec<(usize, CopiedClip)> = clips
            .into_iter()
            .filter_map(|copied| {
                let row = base + copied.lane;
                let kind = &self.project.tracks.get(row)?.kind;
                let fits = match (&copied.content, kind) {
                    (CopiedContent::Midi(_), TrackKind::Instrument(_)) => true,
                    (CopiedContent::Audio(audio), TrackKind::Audio(_)) => {
                        self.project.audio_sources.contains_key(&audio.source)
                    }
                    _ => false,
                };
                fits.then_some((row, copied))
            })
            .collect();
        if landing.is_empty() {
            return Ok(Vec::new());
        }

        self.record(Edit::PasteClips);
        let mut pasted = Vec::with_capacity(landing.len());
        for (row, copied) in landing {
            let id = self.project.next_clip_id();
            let start = at + copied.offset;
            match copied.content {
                CopiedContent::Midi(midi) => {
                    let mut clip = *midi;
                    clip.id = id;
                    clip.start = start;
                    if let Some(inner) = self
                        .project
                        .tracks
                        .get_mut(row)
                        .and_then(|track| track.kind.as_instrument_mut())
                    {
                        inner.clips.push(clip);
                        pasted.push(id);
                    }
                }
                CopiedContent::Audio(audio) => {
                    let mut clip = *audio;
                    clip.id = id;
                    clip.start = start;
                    if let Some(inner) = self
                        .project
                        .tracks
                        .get_mut(row)
                        .and_then(|track| track.kind.as_audio_mut())
                    {
                        inner.clips.push(clip);
                        pasted.push(id);
                    }
                }
            }
        }
        self.invalidate_graph();
        Ok(pasted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{BAR, session, session_with_clip};
    use auris_core::ClipPreset;
    use auris_core::project::ClipRecipe;

    /// A session with two instrument tracks, each carrying one clip a bar apart.
    fn two_tracks() -> (Session, TrackId, TrackId, ClipId, ClipId) {
        let mut session = session();
        let upper = session.add_default_instrument_track("Upper").unwrap();
        let lower = session.add_default_instrument_track("Lower").unwrap();
        let first = session.add_midi_clip(upper, "A", Ticks::ZERO, BAR).unwrap();
        let second = session.add_midi_clip(lower, "B", BAR * 2, BAR).unwrap();
        session
            .add_note(first, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session
            .add_note(second, Note::new(67, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        (session, upper, lower, first, second)
    }

    #[test]
    fn notes_come_back_in_the_shape_they_were_taken_in() {
        let (mut session, _, clip) = session_with_clip();
        // The fixture's two notes are a quarter apart, the second a third above the first.
        assert_eq!(session.copy_notes(clip, &[0, 1]), 2);

        let pasted = session.paste_notes(clip, BAR * 2).unwrap();
        assert_eq!(pasted.len(), 2);
        let notes = &session.midi_clip(clip).unwrap().notes;
        // Relative to each other, not to where they came from: the pair arrives at the position
        // asked for with the gap between them intact.
        assert_eq!(notes[pasted[0]].start, BAR * 2);
        assert_eq!(notes[pasted[1]].start, BAR * 2 + Ticks::QUARTER);
        assert_eq!(notes[pasted[0]].pitch, notes[0].pitch);
        assert_eq!(notes[pasted[1]].pitch, notes[1].pitch);

        // One undo step for the whole paste, not one per note.
        assert_eq!(session.undo(), Some(Edit::PasteNotes));
        assert_eq!(session.midi_clip(clip).unwrap().notes.len(), 2);
    }

    #[test]
    fn the_clipboard_outlives_the_notes_that_were_cut() {
        // The whole of what makes cut different from delete: the material is still somewhere, and
        // it stays there for as many pastes as somebody wants.
        let (mut session, _, clip) = session_with_clip();
        assert_eq!(session.cut_notes(clip, &[0, 1]).unwrap(), 2);
        assert!(session.midi_clip(clip).unwrap().notes.is_empty());
        assert_eq!(session.clipboard().len(), 2);

        session.paste_notes(clip, Ticks::ZERO).unwrap();
        session.paste_notes(clip, BAR).unwrap();
        assert_eq!(session.midi_clip(clip).unwrap().notes.len(), 4);
        assert_eq!(
            session.clipboard().len(),
            2,
            "pasting must not empty the clipboard"
        );

        // A cut is its own undo step, and taking it back brings the notes home.
        session.undo();
        session.undo();
        assert_eq!(session.undo(), Some(Edit::CutNotes));
        assert_eq!(session.midi_clip(clip).unwrap().notes.len(), 2);
    }

    #[test]
    fn a_copy_of_nothing_leaves_what_was_already_there() {
        // A stray ⌘C over an empty selection is how somebody would otherwise lose material they
        // believed was still on the clipboard.
        let (mut session, _, clip) = session_with_clip();
        session.copy_notes(clip, &[0]);
        assert_eq!(session.clipboard().len(), 1);

        assert_eq!(session.copy_notes(clip, &[]), 0);
        assert_eq!(session.copy_notes(clip, &[99]), 0);
        assert_eq!(session.copy_notes(ClipId(9_999), &[0]), 0);
        assert_eq!(session.clipboard().len(), 1);
        // And neither did any of those record an edit.
        assert_eq!(session.cut_notes(clip, &[]).unwrap(), 0);
        assert_eq!(session.undo(), Some(Edit::AddNote));
    }

    #[test]
    fn a_block_of_clips_keeps_its_shape_across_tracks() {
        let (mut session, upper, lower, first, second) = two_tracks();
        assert_eq!(session.copy_clips(&[first, second]), 2);

        // Pasted onto the upper track at bar four: the second clip lands one track down and two
        // bars later, exactly as it sat.
        let pasted = session.paste_clips(upper, BAR * 4).unwrap();
        assert_eq!(pasted.len(), 2);
        let (owner, top) = session.project().midi_clip(pasted[0]).unwrap();
        assert_eq!(owner, upper);
        assert_eq!(top.start, BAR * 4);
        assert_eq!(top.notes.len(), 1, "the notes came with the clip");
        let (owner, bottom) = session.project().midi_clip(pasted[1]).unwrap();
        assert_eq!(owner, lower);
        assert_eq!(bottom.start, BAR * 6);

        // Fresh ids, or the two would be the same clip in two places.
        assert_ne!(pasted[0], first);
        assert_ne!(pasted[1], second);

        assert_eq!(session.undo(), Some(Edit::PasteClips));
        assert!(session.project().midi_clip(pasted[0]).is_none());
    }

    #[test]
    fn a_paste_lands_what_it_can_and_says_what_arrived() {
        let (mut session, upper, _, first, second) = two_tracks();
        session.copy_clips(&[first, second]);

        // Aimed at the lower of the two tracks, the second clip would need a third track. There
        // is none, so it is dropped and the first still lands.
        let lower = session.project().tracks[1].id;
        let pasted = session.paste_clips(lower, BAR * 8).unwrap();
        assert_eq!(pasted.len(), 1);
        assert_eq!(session.project().midi_clip(pasted[0]).unwrap().0, lower);

        // Aimed at an audio track, neither can land — and a paste that lands nothing leaves no
        // undo step behind.
        let audio = session.add_audio_track("Vocals");
        session.forget_history();
        assert!(session.paste_clips(audio, Ticks::ZERO).unwrap().is_empty());
        assert!(
            !session.can_undo(),
            "a paste that landed nothing recorded a step"
        );
        assert!(!session.is_dirty());

        // And a track that is not there at all is a refusal rather than a silent nothing.
        assert!(matches!(
            session.paste_clips(TrackId(9_999), Ticks::ZERO),
            Err(SessionError::UnknownTrack(_))
        ));
        assert_eq!(session.paste_clips(upper, Ticks::ZERO).unwrap().len(), 2);
    }

    #[test]
    fn a_pasted_clip_is_still_the_clip_it_was() {
        // Everything a clip carries beyond its notes — its name, its mute, and the recipe that
        // says it writes itself — travels with it. A pasted generated clip that had forgotten how
        // it was written would be a copy that cannot be regenerated with the original.
        let (mut session, track) = crate::session::fixtures::with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 2,
                ClipRecipe::new(ClipPreset::Bass, 4),
            )
            .unwrap();
        session.set_clip_muted(clip, true).unwrap();
        session.copy_clips(&[clip]);

        let pasted = session.paste_clips(track, BAR * 4).unwrap();
        let copy = session.midi_clip(pasted[0]).unwrap();
        assert!(copy.muted);
        assert_eq!(copy.length, BAR * 2);
        assert_eq!(session.clip_recipe(pasted[0]).map(|r| r.seed), Some(4));
        assert!(session.reroll_clip(pasted[0]).unwrap() > 0);
    }

    #[test]
    fn nothing_on_the_clipboard_is_nothing_pasted() {
        let (mut session, _, clip) = session_with_clip();
        session.forget_history();
        assert!(session.paste_notes(clip, Ticks::ZERO).unwrap().is_empty());
        assert!(!session.can_undo());

        // And the two kinds do not answer for each other: notes on the clipboard are not clips.
        session.copy_notes(clip, &[0]);
        let track = session.project().tracks[0].id;
        assert!(session.paste_clips(track, Ticks::ZERO).unwrap().is_empty());
        assert!(!session.can_undo());
    }
}
