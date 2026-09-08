//! How a clip's notes are performed, separately from what they are.
//!
//! A [`MidiClip`](auris_core::MidiClip) may carry a stack of
//! [`NoteTransform`](auris_core::NoteTransform)s: humanise, swing, transpose, gate — applied as
//! the clip is played or exported, never to the notes it stores. The piano roll keeps showing
//! the text as written, and the stack is how it is *played*; see `crate::guide` for the
//! contract this is half of. These are the commands that edit the stack, and the one that
//! trades it away: freezing writes the performance into the notes and clears it, the same
//! trade [`Session::freeze_clip`] makes with a recipe.

use auris_core::{ClipId, NoteTransform};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

impl Session {
    /// The transform stack a clip is performed through, in the order it is applied.
    pub fn clip_transforms(&self, clip: ClipId) -> Result<&[NoteTransform], SessionError> {
        self.project
            .midi_clip(clip)
            .map(|(_, target)| target.transforms.as_slice())
            .ok_or(SessionError::UnknownClip(clip.0))
    }

    /// Replaces a clip's transform stack.
    ///
    /// The whole stack at once rather than one slot at a time, because the stack is small and a
    /// gesture — a dial, a reorder, a removal — is simplest described by what it leaves behind.
    /// Records as a repeating edit, so a dial still turning folds into one undo step, and a
    /// stack set to what it already is records nothing at all.
    pub fn set_clip_transforms(
        &mut self,
        clip: ClipId,
        transforms: Vec<NoteTransform>,
    ) -> Result<(), SessionError> {
        let Some((_, target)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        if target.transforms == transforms {
            return Ok(());
        }
        self.record_repeating(Edit::SetClipTransforms(clip));
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.transforms = transforms;
        }
        self.invalidate_graph();
        Ok(())
    }

    /// Writes a clip's performance into its notes and clears the stack, returning how many
    /// notes were written.
    ///
    /// What "keep this performance" means, and the same trade freezing a recipe makes: the
    /// result stops being derived from anything, so nothing can move it afterwards — and it
    /// stops being *performed*, so a looped clip's every repeat now rehearses the first pass
    /// instead of wobbling afresh. The visible phrase uses exactly the same clipping as playback.
    /// Notes outside the clip's window retain their stored text; they cannot influence the
    /// frozen performance or be lost merely because an edge currently hides them.
    ///
    /// A clip with no transforms is left untouched and records no step — there is nothing to
    /// keep.
    pub fn freeze_clip_transforms(&mut self, clip: ClipId) -> Result<usize, SessionError> {
        let Some((_, target)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        if target.transforms.is_empty() {
            return Ok(0);
        }
        let bpm = self.project.tempo_map.bpm_at(target.start);
        let transforms = target.transforms.clone();
        let performed = auris_core::performed_note_slots(
            target.playable_notes().collect(),
            &transforms,
            auris_core::PerformanceContext {
                bpm,
                pass: 0,
                start: target.start,
                length: target.length,
                signatures: &self.project.signatures,
            },
        );
        let mut performed = performed.into_iter().map(|slot| {
            slot.map(|mut note| {
                if note.start < target.length {
                    note.length = note.length.min(target.length - note.start);
                }
                note
            })
        });
        // Keep original indices and hidden text; phrase stages preserve original-note order
        // and append their additions, which are appended here after the visible originals.
        let mut notes: Vec<_> = target
            .notes
            .iter()
            .filter_map(|note| {
                if note.start >= auris_core::Ticks::ZERO && note.start < target.length {
                    performed
                        .next()
                        .expect("performance retains every source slot")
                } else {
                    Some(note.clone())
                }
            })
            .collect();
        notes.extend(performed.flatten());
        let count = notes.len();
        self.record(Edit::FreezeClipTransforms);
        if let Some(target) = self.project.midi_clip_mut(clip) {
            target.notes = notes;
            target.transforms.clear();
        }
        self.invalidate_graph();
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use auris_core::Subdivision;
    use auris_core::time::Ticks;

    use super::*;
    use crate::session::fixtures::{session_with_clip, undo_depth};

    fn swing() -> NoteTransform {
        NoteTransform::Swing {
            percent: 67,
            subdivision: Subdivision::Eighth,
        }
    }

    #[test]
    fn freezing_partial_upstrokes_removes_skipped_notes_but_preserves_hidden_text() {
        let (mut session, _, clip) = session_with_clip();
        let held = session.project.midi_clip_mut(clip).unwrap();
        held.notes = [48, 60, 64, 67]
            .map(|pitch| auris_core::Note::new(pitch, Ticks(240), Ticks(240)))
            .to_vec();
        held.notes
            .push(auris_core::Note::new(90, Ticks(-240), Ticks(120)));
        let original = held.notes.clone();
        session
            .set_clip_transforms(
                clip,
                vec![NoteTransform::Strum {
                    settings: auris_core::Strum {
                        up_notes: 2,
                        ..auris_core::Strum::default()
                    },
                }],
            )
            .unwrap();
        let expected: Vec<_> = session
            .midi_clip(clip)
            .unwrap()
            .sounding_notes(120.0)
            .collect();
        assert_eq!(expected.len(), 2);
        session.freeze_clip_transforms(clip).unwrap();
        let held = session.midi_clip(clip).unwrap();
        assert_eq!(held.notes.len(), 3);
        assert_eq!(held.notes[2], original[4]);
        assert_eq!(held.sounding_notes(120.0).collect::<Vec<_>>(), expected);
        session.undo();
        assert_eq!(session.midi_clip(clip).unwrap().notes, original);
        assert!(!session.clip_transforms(clip).unwrap().is_empty());
    }

    #[test]
    fn a_stack_is_set_heard_and_undone() {
        let (mut session, _, clip) = session_with_clip();
        session.set_clip_transforms(clip, vec![swing()]).unwrap();
        assert_eq!(session.clip_transforms(clip).unwrap(), &[swing()]);
        // The text has not moved — that is the whole point of the stack.
        assert_eq!(
            session.project().midi_clip(clip).unwrap().1.notes[1].start,
            Ticks::QUARTER
        );

        assert_eq!(session.undo(), Some(Edit::SetClipTransforms(clip)));
        assert!(session.clip_transforms(clip).unwrap().is_empty());
    }

    #[test]
    fn setting_the_stack_it_already_has_records_nothing() {
        let (mut session, _, clip) = session_with_clip();
        session.set_clip_transforms(clip, vec![swing()]).unwrap();
        let before = undo_depth(&mut session);
        while session.redo().is_some() {}
        session.set_clip_transforms(clip, vec![swing()]).unwrap();
        assert_eq!(undo_depth(&mut session), before);
    }

    #[test]
    fn freezing_writes_the_performance_into_the_text() {
        let (mut session, _, clip) = session_with_clip();
        // The fixture's notes sit on beats; this one is an offbeat eighth the swing will move.
        let off = session
            .add_note(clip, auris_core::Note::new(67, Ticks(480), Ticks(240)))
            .unwrap();
        session.set_clip_transforms(clip, vec![swing()]).unwrap();
        assert_eq!(session.freeze_clip_transforms(clip).unwrap(), 3);

        let (_, frozen) = session.project().midi_clip(clip).unwrap();
        // The offbeat landed where the swing was sending it: 960 × 0.67 = 643.
        assert_eq!(frozen.notes[off].start, Ticks(643));
        assert!(frozen.transforms.is_empty(), "the stack survived freezing");

        // One step back returns both the straight text and the stack.
        assert_eq!(session.undo(), Some(Edit::FreezeClipTransforms));
        let (_, thawed) = session.project().midi_clip(clip).unwrap();
        assert_eq!(thawed.notes[off].start, Ticks(480));
        assert_eq!(thawed.transforms, vec![swing()]);
    }

    #[test]
    fn freezing_nothing_is_a_quiet_no() {
        let (mut session, _, clip) = session_with_clip();
        let before = undo_depth(&mut session);
        assert_eq!(session.freeze_clip_transforms(clip).unwrap(), 0);
        assert_eq!(undo_depth(&mut session), before);
    }

    #[test]
    fn freezing_articulations_keeps_the_heard_notes_and_undo_restores_the_stack() {
        let (mut session, _, clip) = session_with_clip();
        let original = session.midi_clip(clip).unwrap().notes.clone();
        let stack = vec![
            NoteTransform::Gate { amount: 0.5 },
            NoteTransform::Brush { amount: 0.5 },
            NoteTransform::Mute { amount: 0.5 },
            NoteTransform::Stroke {
                spread_ms: 30.0,
                direction: auris_core::StrokeDirection::HighToLow,
            },
            NoteTransform::Humanize {
                amount: 0.5,
                seed: 19,
            },
        ];
        session.set_clip_transforms(clip, stack.clone()).unwrap();
        let expected: Vec<_> = session
            .midi_clip(clip)
            .unwrap()
            .sounding_notes(session.project().tempo_map.bpm_at(Ticks::ZERO))
            .collect();
        assert!(expected.len() > original.len());
        session.freeze_clip_transforms(clip).unwrap();
        let frozen = session.midi_clip(clip).unwrap();
        assert!(frozen.transforms.is_empty());
        assert_eq!(frozen.sounding_notes(120.0).collect::<Vec<_>>(), expected);
        assert_eq!(session.undo(), Some(Edit::FreezeClipTransforms));
        assert_eq!(session.midi_clip(clip).unwrap().notes, original);
        assert_eq!(session.clip_transforms(clip).unwrap(), stack);
    }

    #[test]
    fn an_unknown_clip_is_named_in_the_refusal() {
        let (mut session, _, _) = session_with_clip();
        assert!(matches!(
            session.set_clip_transforms(ClipId(9_999), Vec::new()),
            Err(SessionError::UnknownClip(9_999))
        ));
        assert!(matches!(
            session.freeze_clip_transforms(ClipId(9_999)),
            Err(SessionError::UnknownClip(9_999))
        ));
        assert!(session.clip_transforms(ClipId(9_999)).is_err());
    }

    #[test]
    fn freezing_a_trimmed_phrase_uses_playback_lengths_and_preserves_hidden_notes() {
        let (mut session, _, clip) = session_with_clip();
        let target = session.project.midi_clip_mut(clip).unwrap();
        target.length = Ticks::QUARTER;
        target.notes = vec![
            auris_core::Note::new(60, Ticks::ZERO, Ticks::QUARTER * 2),
            auris_core::Note::new(64, Ticks(1440), Ticks(240)),
        ];
        let hidden = target.notes[1].clone();
        session
            .set_clip_transforms(
                clip,
                vec![
                    NoteTransform::Gate { amount: 0.5 },
                    NoteTransform::Brush { amount: 1.0 },
                ],
            )
            .unwrap();
        let heard: Vec<_> = session
            .midi_clip(clip)
            .unwrap()
            .sounding_notes(120.0)
            .collect();
        session.freeze_clip_transforms(clip).unwrap();
        let frozen = session.midi_clip(clip).unwrap();
        assert_eq!(frozen.sounding_notes(120.0).collect::<Vec<_>>(), heard);
        assert_eq!(frozen.notes[1], hidden);
        assert_eq!(frozen.notes[0].length, Ticks(480));
    }
}
