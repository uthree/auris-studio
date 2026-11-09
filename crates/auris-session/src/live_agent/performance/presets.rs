//! The agent's door to named performance presets.
use super::*;
use auris_core::{PerformancePreset, PresetFamily};

/// What performance_presets does.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PresetOperation {
    /// List built-in and kept presets with family, description and stage kinds. Give name for one preset's full stages; give clip for the family its track suggests.
    List {
        /// Only presets written for this instrument family.
        #[schemars(with = "Option<String>", extend("enum" = ["keys","guitar","bass","drums","strings","brass","woodwinds","synth","other",null]))]
        family: Option<PresetFamily>,
        /// Exact preset name; returns its full stages.
        name: Option<String>,
        /// MIDI clip ID; adds suggested_family from its track's instrument and suggested_preset
        /// from that instrument and the clip's notes.
        clip: Option<u64>,
    },
    /// Replace a clip's (or one drum writer's) stages with a preset's. Pitch and octave stages are skipped on drums.
    Apply {
        /// Stable MIDI clip ID.
        clip: u64,
        /// Exact preset name from list.
        preset: String,
        /// Exact drum writer name; omit for the whole clip.
        drum_voice: Option<String>,
    },
    /// Keep a new preset. Supply exactly one source: clip (its current performance, or drum_voice's) or settings. Never replace a user's preset unless asked.
    Save {
        /// 1..64 characters; built-in names are refused.
        #[schemars(length(min = 1, max = 64))]
        name: String,
        /// Instrument family it is written for.
        #[schemars(with = "String", extend("enum" = ["keys","guitar","bass","drums","strings","brass","woodwinds","synth","other"]))]
        family: PresetFamily,
        /// One line on its sound and use, at most 400 characters.
        #[serde(default)]
        #[schemars(length(max = 400))]
        description: String,
        /// Source MIDI clip ID.
        clip: Option<u64>,
        /// Exact drum writer name within clip.
        drum_voice: Option<String>,
        /// Source settings object with set_performance's fields, e.g. {"gate":0.8,"sustain_ticks":480}.
        #[schemars(with = "Option<serde_json::Map<String, serde_json::Value>>")]
        settings: Option<serde_json::Value>,
        /// Overwrite a kept preset of the same name.
        #[serde(default)]
        replace: bool,
    },
    /// Delete a kept preset by name. Only when the user asked; built-ins cannot be deleted.
    Delete {
        /// Exact kept preset name.
        name: String,
    },
}

impl PresetOperation {
    /// Whether this changes the document or the preset file.
    pub fn is_mutating(&self) -> bool {
        !matches!(self, Self::List { .. })
    }

    /// Whether this removes or overwrites something a person kept.
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, Self::Delete { .. } | Self::Save { replace: true, .. })
    }
}

/// The stage kinds a stack holds, in order, for a compact listing.
fn stage_kinds(stack: &[NoteTransform]) -> Vec<String> {
    stack
        .iter()
        .map(|stage| match stage {
            NoteTransform::ForDrumVoice { voice, transforms } => {
                format!("{voice}: [{}]", stage_kinds(transforms).join(", "))
            }
            other => family(other)
                .and_then(|kind| serde_json::to_value(kind).ok())
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default(),
        })
        .collect()
}

impl Session {
    pub(crate) fn agent_performance_presets(
        &mut self,
        operation: PresetOperation,
    ) -> Result<String, String> {
        match operation {
            PresetOperation::List { family, name, clip } => {
                self.reload_performance_presets();
                if let Some(name) = name {
                    let listed = self
                        .performance_preset(&name)
                        .ok_or_else(|| format!("No preset is named '{}'", name.trim()))?;
                    return Ok(serde_json::json!({
                        "name": listed.preset.name,
                        "builtin": listed.builtin,
                        "family": listed.preset.family,
                        "description": listed.preset.description,
                        "stages": listed.preset.transforms,
                    })
                    .to_string());
                }
                let suggestion = clip
                    .map(|clip| {
                        let (track, clip) = self
                            .project()
                            .midi_clip(ClipId(clip))
                            .ok_or("Unknown MIDI clip ID")?;
                        let track = self.project().track(track).ok_or("Unknown track ID")?;
                        Ok::<_, String>((
                            crate::performance_presets::family_hint(track),
                            crate::performance_presets::suggested_preset(track, clip),
                        ))
                    })
                    .transpose()?;
                let (suggested, suggested_preset) = suggestion.unzip();
                let presets: Vec<_> = self
                    .performance_presets()
                    .into_iter()
                    .filter(|listed| family.is_none_or(|family| listed.preset.family == family))
                    .map(|listed| {
                        serde_json::json!({
                            "name": listed.preset.name,
                            "builtin": listed.builtin,
                            "family": listed.preset.family,
                            "description": listed.preset.description,
                            "stages": stage_kinds(&listed.preset.transforms),
                        })
                    })
                    .collect();
                Ok(serde_json::json!({
                    "suggested_family": suggested,
                    // A starting point for the instrument; a preset that suits the genre better wins.
                    "suggested_preset": suggested_preset.flatten(),
                    "presets": presets,
                    "note": "apply copies a preset's stages into one clip; tune afterwards with set_performance. Presets carry no swing unless a user kept one with it: swing is the song's feel.",
                })
                .to_string())
            }
            PresetOperation::Apply {
                clip,
                preset,
                drum_voice,
            } => {
                let clip = ClipId(clip);
                let listed = self.performance_preset(&preset).ok_or_else(|| {
                    format!("No preset is named '{}'; list names them", preset.trim())
                })?;
                let skipped = match drum_voice.as_deref() {
                    None => self
                        .apply_performance_preset(clip, &preset)
                        .map_err(|e| e.to_string())?,
                    Some(voice) => {
                        self.performance_scope(clip, Some(voice))?;
                        let (mut stack, skipped) = listed.preset.stack_for(clip.0, false);
                        stack.retain(|stage| !matches!(stage, NoteTransform::ForDrumVoice { .. }));
                        let mut outer = self
                            .clip_transforms(clip)
                            .map_err(|e| e.to_string())?
                            .to_vec();
                        scope_into(&mut outer, Some(voice), stack);
                        self.begin_transaction(crate::Edit::SetClipTransforms(clip));
                        let result = self.set_clip_transforms(clip, outer);
                        self.end_transaction();
                        result.map_err(|e| e.to_string())?;
                        skipped
                    }
                };
                let mut report: serde_json::Value = serde_json::from_str(
                    &self.agent_inspect_performance(clip, drum_voice.as_deref())?,
                )
                .map_err(|e| e.to_string())?;
                report["applied_preset"] = listed.preset.name.into();
                report["skipped_stages"] = skipped.into();
                if let Some(warning) = mixed_swing(self.project()) {
                    report["swing_warning"] = warning.into();
                }
                Ok(report.to_string())
            }
            PresetOperation::Save {
                name,
                family,
                description,
                clip,
                drum_voice,
                settings,
                replace,
            } => {
                let transforms = match (clip, settings) {
                    (Some(clip), None) => {
                        self.performance_scope(ClipId(clip), drum_voice.as_deref())?
                            .0
                    }
                    (None, Some(patch)) => {
                        let patch: PerformancePatch =
                            serde_json::from_value(patch).map_err(|e| format!("settings: {e}"))?;
                        if drum_voice.is_some() {
                            return Err("drum_voice needs clip as the source".into());
                        }
                        patch.validate()?;
                        if !has_update(&serde_json::to_value(&patch).map_err(|e| e.to_string())?) {
                            return Err("Supply at least one performance setting".into());
                        }
                        self.patched_stack(Vec::new(), true, 0, &patch)?
                    }
                    _ => return Err("Supply exactly one source: clip or settings".into()),
                };
                let kept = self
                    .keep_performance_preset(
                        PerformancePreset {
                            name,
                            family,
                            description,
                            transforms,
                        },
                        replace,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::json!({
                    "kept": kept.name,
                    "family": kept.family,
                    "stages": stage_kinds(&kept.transforms),
                    "shared": "every project on this machine and the user's preset picker",
                })
                .to_string())
            }
            PresetOperation::Delete { name } => {
                self.forget_performance_preset(&name)
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::json!({"deleted": name.trim()}).to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::live_agent::Command;
    use crate::{Session, SessionOptions};
    use auris_core::NoteTransform;
    use auris_core::time::Ticks;
    use serde_json::{Value, json};

    fn run(session: &mut Session, operation: Value) -> Result<Value, String> {
        let command: Command =
            serde_json::from_value(json!({"action":"performance_presets","operation":operation}))
                .map_err(|e| e.to_string())?;
        session
            .agent_command(command)
            .map(|text| serde_json::from_str(&text).unwrap())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn an_agent_lists_applies_keeps_and_deletes_presets() {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let track = session
            .add_default_instrument_track("Acoustic Guitar")
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Verse", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();

        let listed = run(
            &mut session,
            json!({"kind":"list","family":"guitar","clip":clip.0}),
        )
        .unwrap();
        assert_eq!(listed["suggested_family"], "guitar");
        assert_eq!(listed["suggested_preset"], "acoustic_strum");
        let presets = listed["presets"].as_array().unwrap();
        assert!(presets.iter().all(|p| p["family"] == "guitar"));
        assert!(presets.iter().any(|p| p["name"] == "acoustic_strum"));

        let applied = run(
            &mut session,
            json!({"kind":"apply","clip":clip.0,"preset":"acoustic_strum"}),
        )
        .unwrap();
        assert_eq!(applied["applied_preset"], "acoustic_strum");
        assert!(applied["settings"]["strum"].is_object());
        assert!(applied["settings"]["mute"].as_f64().unwrap() > 0.0);

        let kept = run(
            &mut session,
            json!({"kind":"save","name":"Dusty strum","family":"guitar",
                   "description":"My take","clip":clip.0}),
        )
        .unwrap();
        assert_eq!(kept["kept"], "Dusty strum");
        let from_settings = run(
            &mut session,
            json!({"kind":"save","name":"Half gate","family":"other",
                   "settings":{"gate":0.5,"sustain_ticks":240}}),
        )
        .unwrap();
        assert_eq!(from_settings["stages"], json!(["gate", "sustain"]));
        let detail = run(&mut session, json!({"kind":"list","name":"Half gate"})).unwrap();
        assert_eq!(detail["builtin"], false);
        assert_eq!(detail["stages"][0], json!({"kind":"gate","amount":0.5}));

        assert!(
            run(
                &mut session,
                json!({"kind":"save","name":"Half gate","family":"other","settings":{"gate":0.4}}),
            )
            .is_err(),
            "an existing name needs replace"
        );
        assert!(
            run(
                &mut session,
                json!({"kind":"save","name":"x","family":"other"})
            )
            .is_err()
        );

        run(&mut session, json!({"kind":"delete","name":"Half gate"})).unwrap();
        assert!(run(&mut session, json!({"kind":"delete","name":"piano_ballad"})).is_err());
        assert!(session.performance_preset("Half gate").is_none());
    }

    #[test]
    fn a_drum_writer_takes_a_preset_in_its_own_scope() {
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let track = session
            .add_drum_track("Drums", auris_synth::DrumKit::ID)
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Beat", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        let mut note = auris_core::Note::new(38, Ticks::ZERO, Ticks::QUARTER);
        note.drum_voice = "snare".into();
        session.add_note(clip, note).unwrap();
        run(
            &mut session,
            json!({"kind":"apply","clip":clip.0,"preset":"funk_drums","drum_voice":"snare"}),
        )
        .unwrap();
        let stack = session.clip_transforms(clip).unwrap();
        assert!(matches!(
            stack,
            [NoteTransform::ForDrumVoice { voice, transforms }]
                if voice == "snare" && transforms.len() == 2
        ));
    }

    #[test]
    fn builtin_stages_are_in_the_order_the_panel_and_set_performance_keep() {
        for preset in auris_core::builtin_performance_presets() {
            let ranks: Vec<_> = preset.transforms.iter().map(super::super::rank).collect();
            assert!(ranks.is_sorted(), "{}: {ranks:?}", preset.name);
            let families: Vec<_> = preset.transforms.iter().map(super::super::family).collect();
            let unique: std::collections::BTreeSet<_> =
                families.iter().map(|f| format!("{f:?}")).collect();
            assert_eq!(unique.len(), families.len(), "{}: one of each", preset.name);
        }
    }

    #[test]
    fn presets_are_listed_free_and_deleting_asks_first() {
        use crate::agent_policy::Operation;
        let parse = |operation: Value| {
            Operation::parse(
                "edit_project",
                &json!({"command":{"action":"performance_presets","operation":operation}}),
            )
            .unwrap()
        };
        let list = parse(json!({"kind":"list"}));
        assert!(!list.mutating && !list.confirm);
        let apply = parse(json!({"kind":"apply","clip":1,"preset":"piano_ballad"}));
        assert!(apply.mutating && !apply.confirm);
        assert!(parse(json!({"kind":"delete","name":"mine"})).confirm);
        assert!(
            parse(json!({"kind":"save","name":"m","family":"keys","clip":1,"replace":true}))
                .confirm
        );
    }
}
