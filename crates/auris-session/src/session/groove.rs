//! Capture a portable performance groove from another MIDI clip.
use super::Session;
use crate::{Edit, SessionError};
use auris_core::{
    ClipId, GrooveTemplate, NoteTransform, PerformanceContext, Subdivision, performed_notes,
};

impl Session {
    /// Captures the source clip's first performed pass on a sixteenth-note grid and applies
    /// its timing and relative dynamics to the target without copying notes. The template
    /// is a snapshot: editing or deleting the reference does not change the target later.
    /// Repeating this command refreshes the snapshot, retaining its current strengths.
    pub fn capture_clip_groove(
        &mut self,
        target: ClipId,
        source: ClipId,
    ) -> Result<(), SessionError> {
        let (_, destination) = self
            .project
            .midi_clip(target)
            .ok_or(SessionError::UnknownClip(target.0))?;
        let mut stack = destination.transforms.clone();
        let (_, reference) = self
            .project
            .midi_clip(source)
            .ok_or(SessionError::UnknownClip(source.0))?;
        let notes = performed_notes(
            reference.playable_notes().collect(),
            &reference.transforms,
            PerformanceContext {
                bpm: self.project.tempo_map.bpm_at(reference.start),
                pass: 0,
                start: reference.start,
                length: reference.length,
                signatures: &self.project.signatures,
            },
        );
        let template = GrooveTemplate::capture(
            &reference.name,
            &notes,
            reference.length,
            Subdivision::Sixteenth,
        )
        .ok_or(SessionError::EmptyGroove)?;
        if let Some(stage) = stack
            .iter_mut()
            .find(|t| matches!(t, NoteTransform::Groove { .. }))
        {
            if let NoteTransform::Groove { template: held, .. } = stage {
                *held = template;
            }
        } else {
            // A grid transfer must happen before strokes spread the chord's attacks.
            stack.insert(
                0,
                NoteTransform::Groove {
                    template,
                    timing: 1.0,
                    velocity: 1.0,
                },
            );
        }
        self.begin_transaction(Edit::SetClipTransforms(target));
        let result = self.set_clip_transforms(target, stack);
        self.end_transaction();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{session_with_clip, undo_depth};
    use auris_core::{Note, Ticks};

    #[test]
    fn capture_is_undoable_uses_performed_reference_and_survives_reference_deletion() {
        let (mut session, track, target) = session_with_clip();
        let source = session
            .add_midi_clip(track, "Reference", Ticks(7680), Ticks(1920))
            .unwrap();
        session
            .add_note(source, Note::new(72, Ticks(480), Ticks(120)))
            .unwrap();
        session
            .set_clip_transforms(source, vec![NoteTransform::Lean { ticks: 60 }])
            .unwrap();
        let source_notes = session.project.midi_clip(source).unwrap().1.notes.clone();
        let target_notes = session.project.midi_clip(target).unwrap().1.notes.clone();
        let before = session.clip_transforms(target).unwrap().to_vec();
        session.capture_clip_groove(target, source).unwrap();
        let captured = session.clip_transforms(target).unwrap().to_vec();
        let NoteTransform::Groove { template, .. } = &captured[0] else {
            panic!("groove comes before other stages")
        };
        assert_eq!(template.points[0].offset, Ticks(60));
        assert_eq!(
            session.project.midi_clip(source).unwrap().1.notes,
            source_notes
        );
        assert_eq!(
            session.project.midi_clip(target).unwrap().1.notes,
            target_notes
        );
        session.undo();
        assert_eq!(session.clip_transforms(target).unwrap(), before);
        session.redo();
        assert_eq!(session.clip_transforms(target).unwrap(), captured);
        session.remove_clip(source).unwrap();
        assert_eq!(session.clip_transforms(target).unwrap(), captured);
        let mut stack = captured;
        stack.push(NoteTransform::Expression {
            settings: auris_core::Expression {
                timing: 0.6,
                velocity: 0.8,
                swell: 0.4,
                shared: 0.5,
                seed: 41,
                ..auris_core::Expression::default()
            },
        });
        session.set_clip_transforms(target, stack).unwrap();
        let restored: auris_core::Project =
            serde_json::from_str(&serde_json::to_string(session.project()).unwrap()).unwrap();
        let expected: Vec<_> = restored
            .midi_clip(target)
            .unwrap()
            .1
            .sounding_notes_with_meter(120.0, restored.signatures.clone())
            .collect();
        session.freeze_clip_transforms(target).unwrap();
        assert_eq!(session.project.midi_clip(target).unwrap().1.notes, expected);
        session.undo();
        assert_eq!(
            session.project.midi_clip(target).unwrap().1.notes,
            target_notes
        );
    }

    #[test]
    fn empty_reference_cannot_change_the_stack_or_history() {
        let (mut session, track, target) = session_with_clip();
        let source = session
            .add_midi_clip(track, "Reference", Ticks(7680), Ticks(1920))
            .unwrap();
        let before = undo_depth(&mut session);
        let original = session.clip_transforms(target).unwrap().to_vec();
        assert!(matches!(
            session.capture_clip_groove(target, source),
            Err(SessionError::EmptyGroove)
        ));
        assert_eq!(session.clip_transforms(target).unwrap(), original);
        assert_eq!(undo_depth(&mut session), before);
    }
}
