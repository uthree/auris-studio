//! Applying a [`Fix`] to the document as one undo step.

use auris_core::param::ParamTarget;
use auris_core::time::Ticks;

use super::{Diagnostic, Fix};
use crate::{Edit, Session, SessionError};

impl Session {
    /// Applies one diagnostic's fix as a single undo step.
    ///
    /// `revision` is the revision the diagnostic was found at. A fix names notes by storage
    /// index, and an index found before an edit can name a different note after it, so a stale
    /// fix is refused rather than applied to whatever now sits there.
    pub fn apply_lint_fix(
        &mut self,
        diagnostic: &Diagnostic,
        revision: u64,
    ) -> Result<(), SessionError> {
        if revision != self.revision() {
            return Err(SessionError::LintStale);
        }
        let fix = diagnostic.fix.as_ref().ok_or(SessionError::LintStale)?;
        if matches!(fix, Fix::NormalizeMix) {
            // Records its own step, and opens its own transaction to do it.
            return self.apply_fix(fix);
        }
        self.begin_transaction(Edit::LintFix);
        match self.apply_fix(fix) {
            Ok(()) => {
                self.end_transaction();
                Ok(())
            }
            Err(error) => {
                self.revert_transaction();
                Err(error)
            }
        }
    }

    /// Applies several diagnostics' fixes found at one revision, as one undo step.
    ///
    /// Removals are applied last and together per clip, so one fix cannot shift the indices
    /// another names. A mix normalization, which renders and records its own step, runs after
    /// the note fixes are committed. Returns how many fixes were applied.
    pub fn apply_lint_fixes(
        &mut self,
        diagnostics: &[&Diagnostic],
        revision: u64,
    ) -> Result<usize, SessionError> {
        if revision != self.revision() {
            return Err(SessionError::LintStale);
        }
        let fixes: Vec<&Fix> = diagnostics.iter().filter_map(|d| d.fix.as_ref()).collect();
        if fixes.is_empty() {
            return Ok(0);
        }
        // Removals shift indices, so they go last and in one pass per clip.
        let mut removals: std::collections::BTreeMap<_, Vec<usize>> = Default::default();
        let mut normalize = false;
        self.begin_transaction(Edit::LintFix);
        let result = (|| {
            for fix in &fixes {
                match fix {
                    Fix::RemoveNotes { clip, indices } => {
                        removals.entry(*clip).or_default().extend(indices)
                    }
                    Fix::NormalizeMix => normalize = true,
                    other => self.apply_fix(other)?,
                }
            }
            for (clip, mut indices) in std::mem::take(&mut removals) {
                indices.sort_unstable();
                indices.dedup();
                self.apply_fix(&Fix::RemoveNotes { clip, indices })?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.end_transaction();
                if normalize {
                    self.apply_fix(&Fix::NormalizeMix)?;
                }
                Ok(fixes.len())
            }
            Err(error) => {
                self.revert_transaction();
                Err(error)
            }
        }
    }

    /// One fix, with no transaction of its own: the callers above open it.
    fn apply_fix(&mut self, fix: &Fix) -> Result<(), SessionError> {
        let edit_notes = |session: &mut Self,
                          clip: auris_core::project::ClipId,
                          change: &dyn Fn(
            &mut Vec<auris_core::project::Note>,
        ) -> Result<(), SessionError>| {
            let (_, target) = session
                .project()
                .midi_clip(clip)
                .ok_or(SessionError::UnknownClip(clip.0))?;
            let mut notes = target.notes.clone();
            change(&mut notes)?;
            session.replace_notes(clip, notes)
        };
        let check = |notes: &[auris_core::project::Note], index: usize| {
            if index < notes.len() {
                Ok(())
            } else {
                Err(SessionError::LintStale)
            }
        };
        match fix {
            Fix::RemoveNotes { clip, indices } => edit_notes(self, *clip, &|notes| {
                for index in indices {
                    check(notes, *index)?;
                }
                let mut index = 0;
                notes.retain(|_| {
                    let keep = !indices.contains(&index);
                    index += 1;
                    keep
                });
                Ok(())
            }),
            Fix::TrimNotes { clip, lengths } => edit_notes(self, *clip, &|notes| {
                for (index, length) in lengths {
                    check(notes, *index)?;
                    notes[*index].length = Ticks((*length).max(1));
                }
                Ok(())
            }),
            Fix::SetPitches { clip, pitches } => edit_notes(self, *clip, &|notes| {
                for (index, pitch) in pitches {
                    check(notes, *index)?;
                    notes[*index].pitch = (*pitch).min(127);
                }
                Ok(())
            }),
            Fix::SetVelocity {
                clip,
                indices,
                velocity,
            } => edit_notes(self, *clip, &|notes| {
                for index in indices {
                    check(notes, *index)?;
                    notes[*index].velocity = velocity.clamp(0.0, 1.0);
                }
                Ok(())
            }),
            Fix::ResetFader { track } => {
                self.project()
                    .track(*track)
                    .ok_or(SessionError::UnknownTrack(track.0))?;
                self.set_param(ParamTarget::TrackGain(*track), 0.0);
                Ok(())
            }
            Fix::SetChords { chords, end } => {
                let map = &self.project().harmony.chords;
                // Only a run still blank: a chord written since is somebody's decision.
                let Some(&(first, _)) = chords.first() else {
                    return Err(SessionError::LintStale);
                };
                if map.numeral_at(first).is_some()
                    || map
                        .points()
                        .iter()
                        .any(|point| point.tick > first && point.tick < *end)
                {
                    return Err(SessionError::LintStale);
                }
                let after = map.numeral_at(*end);
                let harmony = self.edit_chords();
                for (at, numeral) in chords {
                    harmony.set_point(*at, Some(*numeral));
                }
                // What came after the run keeps coming after it.
                harmony.set_point(*end, after);
                Ok(())
            }
            Fix::NormalizeMix => {
                self.normalize_mix(crate::TARGET_LUFS, -1.0)?;
                Ok(())
            }
        }
    }
}
