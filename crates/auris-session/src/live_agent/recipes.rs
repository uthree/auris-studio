//! Recipe inspection, partial edits and explicit regeneration through one contract.

use crate::{Edit, Session, prelude::*};
use serde::{Deserialize, Serialize};

/// Partial score-writing controls. Performance controls remain independent.
#[derive(Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecipePatch {
    /// Note density, 0..1.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub density: Option<f32>,
    /// Playing intensity, 0..1.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub intensity: Option<f32>,
    /// Gate ratio, 0..1.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub gate: Option<f32>,
    /// Dynamic variation, 0..1.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub dynamics: Option<f32>,
    /// Syncopation, 0..1.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub syncopation: Option<f32>,
    /// Drum fill amount, 0..1.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub fill: Option<f32>,
    /// Octave offset -2..2 relative to the writer's native register; 0 keeps it.
    #[schemars(range(min = -2, max = 2))]
    pub octave: Option<i32>,
    /// Swing percentage, 50..75.
    #[schemars(range(min = 50, max = 75))]
    pub swing: Option<u8>,
    /// Exact groove name from this schema or the recipe inspector.
    #[schemars(schema_with = "groove_schema")]
    pub groove: Option<String>,
    /// eighth, sixteenth, eighth-triplet or sixteenth-triplet.
    #[schemars(with = "Option<String>", extend("enum" = ["eighth","sixteenth","eighth-triplet","sixteenth-triplet",null]))]
    pub subdivision: Option<Subdivision>,
    /// Space-separated relative scale steps; empty clears the authored motif.
    pub motif: Option<String>,
    /// x hit, X accent, o ghost and ./~/- rest; empty clears authored rhythm.
    #[schemars(length(max = 4096), regex(pattern = r"^[xXo.~ \t|-]*$"))]
    pub rhythm: Option<String>,
    /// Explicit take seed; omitted preserves the current seed.
    pub seed: Option<u64>,
}

fn groove_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let mut names = groove_catalog()
        .iter()
        .map(|groove| serde_json::Value::String(groove.name.to_owned()))
        .collect::<Vec<_>>();
    names.push(serde_json::Value::Null);
    serde_json::json!({"type": ["string", "null"], "enum": names})
        .try_into()
        .expect("groove schema is an object")
}

/// Explicit choice of regeneration take.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Take {
    /// Keep the current seed and rerun the current writer.
    Same,
    /// Increment the current seed.
    Next,
    /// Select an explicit seed.
    Seed {
        /// Desired deterministic seed.
        seed: u64,
    },
}

/// Read controls, change them or explicitly regenerate the stored score.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Read recipe controls, seeds, writer names and hand-edit state.
    Read,
    /// Change only supplied recipe controls and regenerate the selected clips.
    Patch {
        /// Partial controls; omitted fields are preserved.
        settings: RecipePatch,
        /// Explicitly allow overwriting manually edited generated notes.
        #[serde(default)]
        replace_hand_edits: bool,
    },
    /// Rerun the current recipe without changing its controls.
    Regenerate {
        /// Same, next or an explicitly seeded take.
        take: Take,
        /// Explicitly allow overwriting manually edited generated notes.
        #[serde(default)]
        replace_hand_edits: bool,
    },
}

impl Operation {
    /// Whether this operation only reads the document.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Read)
    }
}

impl RecipePatch {
    pub(crate) fn apply(&self, recipe: &mut ClipRecipe) -> Result<(), String> {
        for (name, value, target) in [
            ("density", self.density, &mut recipe.density),
            ("intensity", self.intensity, &mut recipe.intensity),
            ("gate", self.gate, &mut recipe.gate),
            ("dynamics", self.dynamics, &mut recipe.dynamics),
            ("syncopation", self.syncopation, &mut recipe.syncopation),
            ("fill", self.fill, &mut recipe.fill),
        ] {
            if let Some(value) = value {
                if !(0.0..=1.0).contains(&value) {
                    return Err(format!("{name} must be finite within 0..1"));
                }
                *target = value;
            }
        }
        if let Some(value) = self.octave {
            if !(-2..=2).contains(&value) {
                return Err("octave must be -2..2".into());
            }
            recipe.octave = value;
        }
        if let Some(value) = self.swing {
            if !(50..=75).contains(&value) {
                return Err("swing must be 50..75".into());
            }
            recipe.swing = value;
        }
        if let Some(value) = &self.groove {
            if !groove_catalog().iter().any(|groove| groove.name == value) {
                return Err("Unknown groove; read recipe controls first".into());
            }
            recipe.groove = value.clone();
        }
        if let Some(value) = self.subdivision {
            recipe.subdivision = value;
        }
        if let Some(value) = &self.motif {
            if value.len() > 4096 {
                return Err("motif is limited to 4096 bytes".into());
            }
            recipe.motif = if value.trim().is_empty() {
                Vec::new()
            } else {
                parse_motif(value)?
            };
        }
        if let Some(value) = &self.rhythm {
            if value.len() > 4096 {
                return Err("rhythm is limited to 4096 bytes".into());
            }
            recipe.rhythm = if value.trim().is_empty() {
                None
            } else {
                Some(
                    Pattern::parse(value)
                        .ok_or("settings.rhythm uses x (hit), X (accent), o (ghost), ./~/- (rest), with spaces, tabs or | as separators; e.g. x...x...; empty clears it. These are grid steps, not note-duration names. Include at least one step.")?
                        .to_text(),
                )
            };
        }
        if let Some(seed) = self.seed {
            *recipe = recipe.with_seed(seed);
        }
        Ok(())
    }
}

pub(crate) fn summary(recipe: &ClipRecipe) -> serde_json::Value {
    serde_json::json!({"part":recipe.preset.name(),"seed":recipe.seed,"density":recipe.density,"intensity":recipe.intensity,"gate":recipe.gate,"dynamics":recipe.dynamics,"syncopation":recipe.syncopation,"fill":recipe.fill,"octave":recipe.octave,"swing":recipe.swing,"groove":recipe.groove,"subdivision":recipe.subdivision,"motif":recipe.motif,"rhythm":recipe.rhythm,"drum_voices":recipe.drum_voices.iter().map(|voice|serde_json::json!({"name":voice.name,"has_recipe":voice.recipe.is_some()})).collect::<Vec<_>>()})
}

impl Session {
    /// Inspect or revise 1..16 generated clips, keeping all other score and mix state intact.
    pub fn agent_edit_recipe(
        &mut self,
        clips: Vec<u64>,
        drum_voice: Option<String>,
        operation: Operation,
    ) -> Result<String, String> {
        if clips.is_empty() || clips.len() > 16 {
            return Err("clips must contain 1..16 IDs".into());
        }
        let mut seen = std::collections::HashSet::new();
        let mut prepared = Vec::with_capacity(clips.len());
        for clip in clips {
            let clip = ClipId(clip);
            if !seen.insert(clip) {
                return Err("Duplicate clip ID".into());
            }
            let whole = self
                .clip_recipe(clip)
                .ok_or("Clip has no generation recipe")?;
            let original = match &drum_voice {
                Some(name) => whole
                    .drum_voices
                    .iter()
                    .find(|voice| voice.name == *name)
                    .and_then(|voice| voice.recipe.as_deref())
                    .ok_or("Unknown drum writer or missing recipe")?,
                None => whole,
            };
            let mut candidate = original.clone();
            let (replace_hand_edits, regenerate) = match &operation {
                Operation::Read => (false, false),
                Operation::Patch {
                    settings,
                    replace_hand_edits,
                } => {
                    settings.apply(&mut candidate)?;
                    (*replace_hand_edits, candidate != *original)
                }
                Operation::Regenerate {
                    take,
                    replace_hand_edits,
                } => {
                    let seed = match take {
                        Take::Same => candidate.seed,
                        Take::Next => candidate.seed.wrapping_add(1),
                        Take::Seed { seed } => *seed,
                    };
                    candidate = candidate.with_seed(seed);
                    (*replace_hand_edits, true)
                }
            };
            if regenerate && self.clip_hand_edited(clip) && !replace_hand_edits {
                return Err("Generated notes have hand edits; inspect them and explicitly set replace_hand_edits before regeneration".into());
            }
            prepared.push((clip, candidate, regenerate));
        }
        if !operation.is_read_only() {
            self.begin_transaction(Edit::ExternalChanges);
            for (clip, recipe, regenerate) in &prepared {
                if !regenerate {
                    continue;
                }
                let result = match &drum_voice {
                    Some(name) => self.set_drum_voice_recipe(*clip, name, recipe.clone()),
                    None => self.set_clip_recipe(*clip, recipe.clone()),
                };
                if let Err(error) = result {
                    self.revert_transaction();
                    return Err(error.to_string());
                }
            }
            self.end_transaction();
        }
        let clips:Vec<_>=prepared.into_iter().map(|(clip,recipe,_)|serde_json::json!({"clip":clip.0,"recipe":summary(&recipe),"hand_edited":self.clip_hand_edited(clip),"note_count":self.project().midi_clip(clip).map(|(_,clip)|clip.notes.len())})).collect();
        Ok(serde_json::json!({"clips":clips,"revision":self.agent_document_revision()?,"grooves":groove_catalog().iter().map(|groove|groove.name).collect::<Vec<_>>()}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_grid_values_are_typed_and_rhythm_uses_grid_steps() {
        for subdivision in Subdivision::ALL {
            let patch: RecipePatch = serde_json::from_value(serde_json::json!({
                "subdivision": subdivision.name(), "rhythm": "X... | x.o."
            }))
            .unwrap();
            let mut recipe = ClipRecipe::new(ClipPreset::Lead, 41);
            patch.apply(&mut recipe).unwrap();
            assert_eq!(recipe.subdivision, subdivision);
            assert_eq!(recipe.rhythm.as_deref(), Some("X~~~x~o~"));
            assert_eq!(
                serde_json::to_value(&patch).unwrap()["subdivision"],
                subdivision.name()
            );
        }
        assert!(
            serde_json::from_value::<RecipePatch>(serde_json::json!({"subdivision":"quarter"}))
                .is_err()
        );
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .generate_clip(
                track,
                Ticks::ZERO,
                Ticks::QUARTER * 16,
                ClipRecipe::new(ClipPreset::Lead, 41),
            )
            .unwrap();
        let before = session.project().clone();
        let patch = |rhythm: &str| Operation::Patch {
            settings: RecipePatch {
                density: Some(0.2),
                rhythm: Some(rhythm.into()),
                ..Default::default()
            },
            replace_hand_edits: false,
        };
        let error = session
            .agent_edit_recipe(vec![clip.0], None, patch("h h"))
            .unwrap_err();
        assert!(error.contains("grid steps"), "{error}");
        assert_eq!(session.project(), &before);
        session
            .agent_edit_recipe(vec![clip.0], None, patch("x...x..."))
            .unwrap();
        assert_eq!(
            session.clip_recipe(clip).unwrap().rhythm.as_deref(),
            Some("x~~~x~~~")
        );
        session
            .agent_edit_recipe(vec![clip.0], None, patch(""))
            .unwrap();
        assert!(session.clip_recipe(clip).unwrap().rhythm.is_none());
    }

    #[test]
    fn recipe_batches_keep_seeds_protect_hand_edits_and_undo_together() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        session
            .agent_command(super::super::Command::EditHarmony {
                start_bar: 1,
                bars: 4,
                key: Some("C major".into()),
                chords: Some("| I | IV | V | I |".into()),
                section: None,
                clear_chords: false,
            })
            .unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let first = session
            .generate_clip(
                track,
                Ticks::ZERO,
                Ticks::QUARTER * 16,
                ClipRecipe::new(ClipPreset::Lead, 41),
            )
            .unwrap();
        let second = session
            .generate_clip(
                track,
                Ticks::ZERO,
                Ticks::QUARTER * 16,
                ClipRecipe::new(ClipPreset::Chords, 82),
            )
            .unwrap();
        let before = session.project().clone();
        let patch = || Operation::Patch {
            settings: RecipePatch {
                gate: Some(0.2),
                ..Default::default()
            },
            replace_hand_edits: false,
        };
        session
            .agent_edit_recipe(vec![first.0, second.0], None, patch())
            .unwrap();
        assert_eq!(session.clip_recipe(first).unwrap().seed, 41);
        assert_eq!(session.clip_recipe(second).unwrap().seed, 82);
        assert_eq!(session.clip_recipe(second).unwrap().gate, 0.2);
        session.undo();
        assert_eq!(session.project(), &before);
        assert!(
            session
                .agent_edit_recipe(vec![first.0, u64::MAX], None, patch())
                .is_err()
        );
        assert_eq!(session.project(), &before);
        let mut notes = session.project().midi_clip(first).unwrap().1.notes.clone();
        notes[0].velocity = 0.123;
        session.replace_notes(first, notes).unwrap();
        let authored = session.project().clone();
        assert!(
            session
                .agent_edit_recipe(
                    vec![first.0],
                    None,
                    Operation::Regenerate {
                        take: Take::Next,
                        replace_hand_edits: false
                    }
                )
                .is_err()
        );
        assert_eq!(session.project(), &authored);
        session
            .agent_edit_recipe(
                vec![first.0],
                None,
                Operation::Regenerate {
                    take: Take::Next,
                    replace_hand_edits: true,
                },
            )
            .unwrap();
        assert_eq!(session.clip_recipe(first).unwrap().seed, 42);
        session.undo();
        assert_eq!(session.project(), &authored);
    }
}
