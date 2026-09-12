//! Step editing of a generated clip, with independent drum voices.

use super::Session;
use crate::error::SessionError;
use auris_compose::rhythm::{Accent, Grid, Pattern};
use auris_core::{ClipId, ClipPreset, ClipRecipe, DrumRole, DrumVoiceRecipe};

/// One instrument's rhythm across the first bar of a generated clip.
#[derive(Clone, Debug)]
pub struct RhythmRow {
    /// Stable writer identity; empty for a melodic or single-drum clip.
    pub voice: String,
    /// Musical drum role, or none for melodic material.
    pub role: Option<DrumRole>,
    /// Whether the writer still chooses the rhythm automatically.
    pub automatic: bool,
    /// Fixed arrangement accents are displayed but have no rhythmic writer.
    pub editable: bool,
    /// Hits in subdivision order. Authored accents remain intact when other cells change.
    pub steps: Vec<bool>,
}

/// The current bar's rhythm, ready for a frontend to present as a step grid.
#[derive(Clone, Debug)]
pub struct RhythmGrid {
    /// Steps between felt beats.
    pub steps_per_beat: usize,
    /// Rows in musical writer order.
    pub rows: Vec<RhythmRow>,
}

/// Makes a basic kit's existing roles independently editable without changing their settings.
fn expanded_recipe(mut recipe: ClipRecipe) -> ClipRecipe {
    if recipe.preset != ClipPreset::Drums {
        recipe.drum_voices.clear();
    }
    if recipe.preset == ClipPreset::Drums && recipe.drum_voices.is_empty() {
        for role in auris_compose::roles_of(recipe.preset) {
            let (Some(drum_role), Some(preset)) =
                (role.drum_role(), auris_compose::preset_of(*role))
            else {
                continue;
            };
            let part = auris_compose::PartSpec::of_role(role.name(), *role);
            let note = match &recipe.drum_map {
                Some(map) => match map.voices.get(&drum_role) {
                    Some(note) => *note,
                    None => continue,
                },
                None => recipe.drum_note.or_else(|| part.drum_note()).unwrap_or(0),
            };
            let mut writer = recipe.clone();
            writer.preset = preset;
            writer.drum_voices.clear();
            writer.drum_map = None;
            writer.drum_note = Some(note);
            recipe.drum_voices.push(DrumVoiceRecipe {
                name: role.name().into(),
                role: drum_role,
                note,
                recipe: Some(Box::new(writer)),
                fixed_notes: Vec::new(),
            });
        }
    }
    recipe
}

impl Session {
    /// Reads the first bar without freezing an automatically generated rhythm.
    pub fn clip_rhythm_grid(&self, clip: ClipId) -> Result<RhythmGrid, SessionError> {
        let midi = self
            .midi_clip(clip)
            .ok_or(SessionError::UnknownClip(clip.0))?;
        let recipe = expanded_recipe(
            midi.recipe
                .clone()
                .ok_or(SessionError::NotGenerated(clip.0))?,
        );
        let grid = Grid::new(
            self.signature_at(midi.start),
            if recipe.preset.is_drums() {
                4
            } else {
                recipe.subdivision.steps_per_beat()
            },
        );
        let steps = grid.steps_per_bar();
        let make_row = |voice: String, role, writer: Option<&ClipRecipe>, note: Option<u8>| {
            let pattern = writer
                .and_then(|writer| writer.rhythm.as_deref())
                .and_then(Pattern::parse);
            let mut hits = vec![false; steps];
            if let Some(pattern) = &pattern {
                for (index, hit) in hits.iter_mut().enumerate() {
                    *hit = pattern.at(index).is_some();
                }
            } else {
                for played in &midi.notes {
                    let belongs = voice.is_empty()
                        || played.drum_voice == voice
                        || (played.drum_voice.is_empty() && note == Some(played.pitch));
                    if belongs
                        && played.start < grid.bar_ticks()
                        && let Some(hit) = hits.get_mut(grid.step_of(played.start))
                    {
                        *hit = true;
                    }
                }
            }
            RhythmRow {
                voice,
                role,
                automatic: writer.is_some() && pattern.is_none(),
                editable: writer.is_some(),
                steps: hits,
            }
        };
        let rows = if recipe.drum_voices.is_empty() {
            // An empty mapping has no playable drum rows.
            if recipe.preset == ClipPreset::Drums {
                Vec::new()
            } else {
                let role = auris_compose::roles_of(recipe.preset)
                    .first()
                    .and_then(|role| role.drum_role());
                if role.is_some_and(|role| {
                    recipe
                        .drum_map
                        .as_ref()
                        .is_some_and(|map| !map.voices.contains_key(&role))
                }) {
                    Vec::new()
                } else {
                    vec![make_row(
                        String::new(),
                        role,
                        Some(&recipe),
                        recipe.drum_note,
                    )]
                }
            }
        } else {
            recipe
                .drum_voices
                .iter()
                .filter(|voice| {
                    recipe
                        .drum_map
                        .as_ref()
                        .is_none_or(|map| map.voices.contains_key(&voice.role))
                })
                .map(|voice| {
                    make_row(
                        voice.name.clone(),
                        Some(voice.role),
                        voice.recipe.as_deref(),
                        Some(voice.note),
                    )
                })
                .collect()
        };
        Ok(RhythmGrid {
            steps_per_beat: grid.steps_per_beat(),
            rows,
        })
    }

    /// Toggles one step, adopting the visible first-bar rhythm on the first manual edit.
    /// Only the selected drum writer is regenerated. Each click is one undo step.
    pub fn toggle_clip_rhythm_step(
        &mut self,
        clip: ClipId,
        voice: &str,
        step: usize,
    ) -> Result<usize, SessionError> {
        let grid = self.clip_rhythm_grid(clip)?;
        let row = grid
            .rows
            .iter()
            .find(|row| row.voice == voice && row.editable)
            .ok_or(SessionError::InvalidRhythm)?;
        if step >= row.steps.len() {
            return Err(SessionError::InvalidRhythm);
        }
        let recipe = expanded_recipe(
            self.clip_recipe(clip)
                .cloned()
                .ok_or(SessionError::NotGenerated(clip.0))?,
        );
        let writer = if voice.is_empty() {
            &recipe
        } else {
            recipe
                .drum_voices
                .iter()
                .find(|part| part.name == voice)
                .and_then(|part| part.recipe.as_deref())
                .ok_or(SessionError::InvalidRhythm)?
        };
        let previous = writer.rhythm.as_deref().and_then(Pattern::parse);
        let mut pattern = Pattern {
            steps: row
                .steps
                .iter()
                .enumerate()
                .map(|(index, hit)| {
                    previous.as_ref().map_or_else(
                        || hit.then_some(Accent::Normal),
                        |pattern| pattern.at(index),
                    )
                })
                .collect(),
        };
        pattern.steps[step] = if pattern.steps[step].is_some() {
            None
        } else {
            Some(Accent::Normal)
        };
        self.set_rhythm_row(clip, voice, Some(pattern.to_text()), recipe)
    }

    /// Restores automatic rhythm for one row without changing the other drum voices.
    pub fn reset_clip_rhythm_row(
        &mut self,
        clip: ClipId,
        voice: &str,
    ) -> Result<usize, SessionError> {
        let recipe = expanded_recipe(
            self.clip_recipe(clip)
                .cloned()
                .ok_or(SessionError::NotGenerated(clip.0))?,
        );
        self.set_rhythm_row(clip, voice, None, recipe)
    }

    fn set_rhythm_row(
        &mut self,
        clip: ClipId,
        voice: &str,
        rhythm: Option<String>,
        mut recipe: ClipRecipe,
    ) -> Result<usize, SessionError> {
        if voice.is_empty() && recipe.drum_voices.is_empty() {
            if recipe.rhythm == rhythm {
                return Ok(self.midi_clip(clip).map_or(0, |midi| midi.notes.len()));
            }
            recipe.rhythm = rhythm;
            return self.set_clip_recipe(clip, recipe);
        }
        let mut writer = recipe
            .drum_voices
            .iter()
            .find(|part| part.name == voice)
            .and_then(|part| part.recipe.as_deref())
            .cloned()
            .ok_or(SessionError::InvalidRhythm)?;
        if writer.rhythm == rhythm {
            return Ok(self.midi_clip(clip).map_or(0, |midi| midi.notes.len()));
        }
        writer.rhythm = rhythm;
        // The kit has independent patterns now, rather than a shared override.
        recipe.rhythm = None;
        self.set_drum_voice_recipe_in(clip, voice, writer, recipe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{BAR, with_a_progression};
    use auris_core::{Note, Ticks, TimeSignature};

    #[test]
    fn melodic_grid_adopts_visible_hits_and_undo_restores_automatic() {
        let (mut session, track) = with_a_progression();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                BAR * 4,
                ClipRecipe::new(ClipPreset::Arp, 1),
            )
            .unwrap();
        let original = session.midi_clip(clip).unwrap().clone();
        let before = session.clip_rhythm_grid(clip).unwrap();
        session.forget_history();
        session.reset_clip_rhythm_row(clip, "").unwrap();
        assert!(session.undo().is_none());
        session.toggle_clip_rhythm_step(clip, "", 1).unwrap();
        let after = session.clip_rhythm_grid(clip).unwrap();
        assert!(!after.rows[0].automatic);
        let mut expected = before.rows[0].steps.clone();
        expected[1] = !expected[1];
        assert_eq!(after.rows[0].steps, expected);
        session.undo().unwrap();
        assert_eq!(session.midi_clip(clip), Some(&original));
        session.redo().unwrap();
        session.reset_clip_rhythm_row(clip, "").unwrap();
        assert_eq!(session.midi_clip(clip), Some(&original));
    }

    #[test]
    fn editing_a_basic_kit_row_keeps_other_instruments_and_undo_is_atomic() {
        let (mut session, _) = with_a_progression();
        let track = session.project.add_drum_track("Kit", "auris.synth.drumkit");
        let mut recipe = ClipRecipe::new(ClipPreset::Drums, 2);
        recipe.drum_map = Some(auris_core::DrumMap::from_voices([
            (DrumRole::Kick, 36),
            (DrumRole::Snare, 38),
            (DrumRole::ClosedHat, 42),
        ]));
        let clip = session
            .generate_clip(track, Ticks::ZERO, BAR * 4, recipe)
            .unwrap();
        let original = session.midi_clip(clip).unwrap().clone();
        let grid = session.clip_rhythm_grid(clip).unwrap();
        assert_eq!(grid.rows.len(), 3);
        assert_eq!(grid.rows[0].steps.len(), 16);
        let voice = grid.rows[0].voice.clone();
        let others = |notes: &[Note]| {
            notes
                .iter()
                .filter(|note| note.drum_voice != voice)
                .cloned()
                .collect::<Vec<_>>()
        };
        session.toggle_clip_rhythm_step(clip, &voice, 3).unwrap();
        assert_eq!(
            others(&session.midi_clip(clip).unwrap().notes),
            others(&original.notes)
        );
        let changed = session.clip_rhythm_grid(clip).unwrap();
        assert!(!changed.rows[0].automatic);
        assert!(changed.rows[1..].iter().all(|row| row.automatic));
        session.reset_clip_rhythm_row(clip, &voice).unwrap();
        assert!(session.clip_rhythm_grid(clip).unwrap().rows[0].automatic);
        assert_eq!(
            others(&session.midi_clip(clip).unwrap().notes),
            others(&original.notes)
        );
        session.undo().unwrap();
        session.undo().unwrap();
        assert_eq!(session.midi_clip(clip), Some(&original));
    }

    #[test]
    fn meter_and_subdivision_determine_columns_and_invalid_cells_do_not_edit() {
        let (mut session, track) = with_a_progression();
        session.set_signature_at(Ticks::ZERO, TimeSignature::new(3, 4));
        let mut recipe = ClipRecipe::new(ClipPreset::Arp, 1);
        recipe.subdivision = auris_core::Subdivision::Sixteenth;
        let clip = session
            .generate_clip(track, Ticks::ZERO, BAR, recipe)
            .unwrap();
        let grid = session.clip_rhythm_grid(clip).unwrap();
        assert_eq!(grid.rows[0].steps.len(), 12);
        assert_eq!(grid.steps_per_beat, 4);
        let before = session.midi_clip(clip).unwrap().clone();
        assert!(session.toggle_clip_rhythm_step(clip, "", 12).is_err());
        assert!(session.toggle_clip_rhythm_step(clip, "missing", 0).is_err());
        assert_eq!(session.midi_clip(clip), Some(&before));
    }
}
