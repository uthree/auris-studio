//! Typed, partial edits of the open clip's non-destructive performance.
mod effects;
#[cfg(test)]
mod tests;
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn expression(stack: &[NoteTransform], seed: u64) -> Expression {
    stack
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Expression { settings } => Some(settings.clone()),
            NoteTransform::Humanize { amount, seed } => Some(Expression {
                timing: *amount,
                velocity: *amount,
                seed: *seed,
                ..Default::default()
            }),
            _ => None,
        })
        .unwrap_or(Expression {
            seed,
            ..Default::default()
        })
}

fn strum(stack: &[NoteTransform]) -> Strum {
    stack
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Strum { settings } => Some(settings.clone()),
            NoteTransform::Stroke {
                spread_ms,
                direction,
            } => Some(Strum {
                spread_ms: *spread_ms,
                direction: *direction,
                clock: StrumClock::Attacks,
                ..Default::default()
            }),
            _ => None,
        })
        .unwrap_or_default()
}

fn ghost(stack: &[NoteTransform], seed: u64) -> GhostNotes {
    stack
        .iter()
        .find_map(|stage| match stage {
            NoteTransform::Ghost { settings } => Some(settings.clone()),
            NoteTransform::Brush { amount } => Some(GhostNotes {
                density: 1.0,
                velocity: 0.3 * amount,
                pattern: GhostPattern::Sixteenths,
                preserve_rests: false,
                seed,
                ..Default::default()
            }),
            _ => None,
        })
        .unwrap_or(GhostNotes {
            seed,
            ..Default::default()
        })
}

fn family(stage: &NoteTransform) -> Option<ResetStage> {
    Some(match stage {
        NoteTransform::Swing { .. } => ResetStage::Swing,
        NoteTransform::Gate { .. } => ResetStage::Gate,
        NoteTransform::Sustain { .. } => ResetStage::Sustain,
        NoteTransform::Expression { .. } | NoteTransform::Humanize { .. } => ResetStage::Expression,
        NoteTransform::Strum { .. } | NoteTransform::Stroke { .. } => ResetStage::Strum,
        NoteTransform::Ghost { .. } | NoteTransform::Brush { .. } => ResetStage::Ghost,
        NoteTransform::Mute { .. } => ResetStage::Mute,
        NoteTransform::Slide { .. } => ResetStage::Slide,
        NoteTransform::Pitch { .. } => ResetStage::Pitch,
        NoteTransform::Octaves { .. } => ResetStage::Octaves,
        NoteTransform::Groove { .. } => ResetStage::Groove,
        NoteTransform::Transpose { .. } => ResetStage::Transpose,
        NoteTransform::Lean { .. } => ResetStage::Lean,
        NoteTransform::ForDrumVoice { .. } => return None,
    })
}

fn rank(stage: &NoteTransform) -> usize {
    match stage {
        NoteTransform::Groove { .. } | NoteTransform::Swing { .. } => 0,
        NoteTransform::Transpose { .. } => 1,
        NoteTransform::Gate { .. } => 2,
        NoteTransform::Brush { .. } | NoteTransform::Ghost { .. } => 3,
        NoteTransform::Slide { .. } => 4,
        NoteTransform::Mute { .. } => 5,
        NoteTransform::Strum { .. } | NoteTransform::Stroke { .. } => 6,
        NoteTransform::Lean { .. } | NoteTransform::ForDrumVoice { .. } => 7,
        NoteTransform::Humanize { .. } | NoteTransform::Expression { .. } => 8,
        NoteTransform::Sustain { .. } => 9,
        NoteTransform::Pitch { .. } => 10,
        NoteTransform::Octaves { .. } => 11,
    }
}

fn replace(stack: &mut Vec<NoteTransform>, kind: ResetStage, replacement: Option<NoteTransform>) {
    let held = stack.iter().position(|stage| family(stage) == Some(kind));
    match (held, replacement) {
        (Some(at), Some(stage)) => stack[at] = stage,
        (Some(at), None) => {
            stack.remove(at);
        }
        (None, Some(stage)) => {
            let at = stack
                .iter()
                .position(|other| rank(other) > rank(&stage))
                .unwrap_or(stack.len());
            stack.insert(at, stage);
        }
        (None, None) => {}
    }
}

fn merge<T: Serialize + serde::de::DeserializeOwned>(
    settings: T,
    patch: &impl Serialize,
) -> Result<T, String> {
    let mut held = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    let updates = serde_json::to_value(patch).map_err(|e| e.to_string())?;
    if let (Some(held), Some(updates)) = (held.as_object_mut(), updates.as_object()) {
        for (key, value) in updates {
            if !value.is_null() && held.contains_key(key) {
                held.insert(key.clone(), value.clone());
            }
        }
    }
    serde_json::from_value(held).map_err(|e| e.to_string())
}

fn has_update(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Object(values) => values.values().any(has_update),
        serde_json::Value::Array(values) => !values.is_empty(),
        _ => true,
    }
}

pub(crate) fn measure_arrangement_performance(
    clip: &MidiClip,
    project: &Project,
    voice: Option<&str>,
) -> serde_json::Value {
    let stack = voice
        .and_then(|voice| {
            clip.transforms.iter().find_map(|stage| match stage {
                NoteTransform::ForDrumVoice {
                    voice: held,
                    transforms,
                } if held == voice => Some(transforms.as_slice()),
                _ => None,
            })
        })
        .unwrap_or(&clip.transforms);
    effects::measure(clip, stack, voice, project)
}

impl Session {
    fn performance_scope(
        &self,
        clip: ClipId,
        voice: Option<&str>,
    ) -> Result<(Vec<NoteTransform>, bool), String> {
        let (track, target) = self
            .project()
            .midi_clip(clip)
            .ok_or("Unknown MIDI clip ID")?;
        let track = self.project().track(track).ok_or("Unknown track ID")?;
        let melodic = track.kind.as_instrument().is_some() && !track.kind.is_drum();
        let Some(voice) = voice else {
            return Ok((target.transforms.clone(), melodic));
        };
        if !track.kind.is_drum() || voice.trim().is_empty() {
            return Err("drum_voice requires a drum clip and an exact nonempty writer name".into());
        }
        let scoped = target.transforms.iter().find_map(|stage| match stage {
            NoteTransform::ForDrumVoice {
                voice: held,
                transforms,
            } if held == voice => Some(transforms.clone()),
            _ => None,
        });
        let known = scoped.is_some()
            || target.notes.iter().any(|note| note.drum_voice == voice)
            || target
                .recipe
                .as_ref()
                .is_some_and(|recipe| recipe.drum_voices.iter().any(|part| part.name == voice));
        if !known {
            return Err("Unknown drum writer; use exact drum_voice names from inspect_performance or read_notes".into());
        }
        Ok((scoped.unwrap_or_default(), false))
    }

    pub(super) fn agent_inspect_performance(
        &self,
        clip: ClipId,
        voice: Option<&str>,
    ) -> Result<String, String> {
        let (stack, melodic) = self.performance_scope(clip, voice)?;
        let (_, target) = self
            .project()
            .midi_clip(clip)
            .ok_or("Unknown MIDI clip ID")?;
        let scalar = |kind: ResetStage| stack.iter().find(|stage| family(stage) == Some(kind));
        let mut pitch = serde_json::to_value(
            stack
                .iter()
                .find_map(|s| match s {
                    NoteTransform::Pitch { settings } => Some(settings.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?;
        let contour = pitch
            .as_object_mut()
            .and_then(|p| p.remove("volume_contour"));
        let swing = scalar(ResetStage::Swing);
        let groove = scalar(ResetStage::Groove);
        let voices: std::collections::BTreeSet<_> = target
            .notes
            .iter()
            .filter(|n| !n.drum_voice.is_empty())
            .map(|n| n.drum_voice.as_str())
            .chain(
                target
                    .recipe
                    .iter()
                    .flat_map(|r| r.drum_voices.iter().map(|v| v.name.as_str())),
            )
            .chain(target.transforms.iter().filter_map(|s| match s {
                NoteTransform::ForDrumVoice { voice, .. } => Some(voice.as_str()),
                _ => None,
            }))
            .collect();
        let note_effects = effects::measure(target, &stack, voice, self.project());
        let pitch_effects = note_effects
            .get("pitch_effects")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        Ok(serde_json::json!({
            "clip": clip.0,
            "drum_voice": voice,
            "available_drum_voices": voices.iter().take(64).collect::<Vec<_>>(),
            "drum_voice_count": voices.len(),
            "melodic_pitch_supported": melodic,
            "stage_order": stack.iter().filter_map(family).take(64).collect::<Vec<_>>(),
            "stage_count": stack.len(),
            "note_effects": note_effects,
            "pitch_effects": pitch_effects,
            "units": {
                "ratios": "0..1",
                "swing_percent": "50..75 percent",
                "times": "milliseconds",
                "pitch_depths": "semitones",
                "lean_ticks": "960 ticks per quarter note",
                "sustain_ticks": "960 ticks per quarter note"
            },
            "settings": {
                "humanize": match scalar(ResetStage::Expression) {
                    Some(NoteTransform::Humanize { amount, .. }) => *amount,
                    Some(NoteTransform::Expression { settings }) => settings.timing.max(settings.velocity),
                    _ => 0.0
                },
                "swing_percent": match swing {
                    Some(NoteTransform::Swing { percent, .. }) => *percent,
                    _ => 50
                },
                "swing_grid": match swing {
                    Some(NoteTransform::Swing { subdivision, .. }) => *subdivision,
                    _ => Subdivision::Sixteenth
                },
                "gate": match scalar(ResetStage::Gate) {
                    Some(NoteTransform::Gate { amount }) => *amount,
                    _ => 1.0
                },
                "sustain_ticks": match scalar(ResetStage::Sustain) {
                    Some(NoteTransform::Sustain { ticks }) => *ticks,
                    _ => 0
                },
                "mute": match scalar(ResetStage::Mute) {
                    Some(NoteTransform::Mute { amount }) => *amount,
                    _ => 0.0
                },
                "brush": if scalar(ResetStage::Ghost).is_some() {
                    (ghost(&stack, clip.0).velocity / 0.3).clamp(0.0, 1.0)
                } else { 0.0 },
                "slide": match scalar(ResetStage::Slide) {
                    Some(NoteTransform::Slide { amount }) => *amount,
                    _ => 0.0
                },
                "transpose": match scalar(ResetStage::Transpose) {
                    Some(NoteTransform::Transpose { semitones }) => *semitones,
                    _ => 0
                },
                "lean_ticks": match scalar(ResetStage::Lean) {
                    Some(NoteTransform::Lean { ticks }) => *ticks,
                    _ => 0
                },
                "octave_above": match scalar(ResetStage::Octaves) {
                    Some(NoteTransform::Octaves { above, .. }) => *above,
                    _ => 0.0
                },
                "octave_below": match scalar(ResetStage::Octaves) {
                    Some(NoteTransform::Octaves { below, .. }) => *below,
                    _ => 0.0
                },
                "expression": scalar(ResetStage::Expression).map(|_| expression(&stack, clip.0)),
                "strum": scalar(ResetStage::Strum).map(|_| strum(&stack)),
                "ghost": scalar(ResetStage::Ghost).map(|_| ghost(&stack, clip.0)),
                "pitch": scalar(ResetStage::Pitch).map(|_| pitch),
                "groove": match groove {
                    Some(NoteTransform::Groove { timing, velocity, .. }) =>
                        Some(serde_json::json!({"timing": timing, "velocity": velocity})),
                    _ => None
                }
            },
            "groove_template": match groove {
                Some(NoteTransform::Groove { template, .. }) => Some(serde_json::json!({
                    "name": template.name,
                    "length_ticks": template.length,
                    "subdivision": template.subdivision,
                    "point_count": template.points.len()
                })),
                _ => None
            },
            "volume_contour": {
                "point_count": contour.as_ref().and_then(|c| c.as_array()).map_or(0, Vec::len),
                "preserved": true
            },
            "non_destructive": true
        }).to_string())
    }

    pub(super) fn agent_set_performance(
        &mut self,
        clip: ClipId,
        voice: Option<&str>,
        patch: PerformancePatch,
    ) -> Result<String, String> {
        let outer = self.prepare_agent_performance(clip, voice, patch)?;
        self.begin_transaction(crate::Edit::SetClipTransforms(clip));
        let result = self
            .set_clip_transforms(clip, outer)
            .map_err(|e| e.to_string());
        self.end_transaction();
        result?;
        self.agent_inspect_performance(clip, voice)
    }

    pub(crate) fn prepare_agent_performance(
        &self,
        clip: ClipId,
        voice: Option<&str>,
        patch: PerformancePatch,
    ) -> Result<Vec<NoteTransform>, String> {
        patch.validate()?;
        if !has_update(&serde_json::to_value(&patch).map_err(|e| e.to_string())?) {
            return Err(
                "Supply at least one performance setting or an explicit reset stage".into(),
            );
        }
        let (mut stack, melodic) = self.performance_scope(clip, voice)?;
        if !melodic
            && (patch.pitch.is_some()
                || patch.octave_above.is_some()
                || patch.octave_below.is_some())
        {
            return Err(
                "Automatic pitch and octave controls require a melodic instrument clip".into(),
            );
        }
        stack.retain(|stage| !patch.reset.iter().any(|kind| family(stage) == Some(*kind)));
        if let Some(amount) = patch.humanize {
            if stack
                .iter()
                .any(|s| matches!(s, NoteTransform::Expression { .. }))
            {
                let mut settings = expression(&stack, clip.0);
                let peak = settings.timing.max(settings.velocity);
                if peak > 0.0 {
                    settings.timing *= amount / peak;
                    settings.velocity *= amount / peak;
                } else {
                    settings.timing = amount;
                    settings.velocity = amount;
                }
                replace(
                    &mut stack,
                    ResetStage::Expression,
                    Some(NoteTransform::Expression { settings }),
                );
            } else {
                let seed = match stack
                    .iter()
                    .find(|s| matches!(s, NoteTransform::Humanize { .. }))
                {
                    Some(NoteTransform::Humanize { seed, .. }) => *seed,
                    _ => clip.0,
                };
                replace(
                    &mut stack,
                    ResetStage::Expression,
                    (amount > 0.0).then_some(NoteTransform::Humanize { amount, seed }),
                );
            }
        }
        if let Some(update) = &patch.expression {
            let settings = merge(expression(&stack, clip.0), update)?;
            replace(
                &mut stack,
                ResetStage::Expression,
                Some(NoteTransform::Expression { settings }),
            );
        }
        if patch.swing_percent.is_some() || patch.swing_grid.is_some() {
            let (percent, subdivision) = stack
                .iter()
                .find_map(|s| match s {
                    NoteTransform::Swing {
                        percent,
                        subdivision,
                    } => Some((*percent, *subdivision)),
                    _ => None,
                })
                .unwrap_or((50, Subdivision::Sixteenth));
            let percent = patch.swing_percent.unwrap_or(percent);
            replace(
                &mut stack,
                ResetStage::Swing,
                (percent > 50).then_some(NoteTransform::Swing {
                    percent,
                    subdivision: patch.swing_grid.unwrap_or(subdivision),
                }),
            );
        }
        if let Some(amount) = patch.gate {
            replace(
                &mut stack,
                ResetStage::Gate,
                (amount < 1.0).then_some(NoteTransform::Gate { amount }),
            );
        }
        if let Some(ticks) = patch.sustain_ticks {
            replace(
                &mut stack,
                ResetStage::Sustain,
                (ticks > 0).then_some(NoteTransform::Sustain { ticks }),
            );
        }
        if let Some(amount) = patch.mute {
            replace(
                &mut stack,
                ResetStage::Mute,
                (amount > 0.0).then_some(NoteTransform::Mute { amount }),
            );
        }
        if let Some(amount) = patch.slide {
            replace(
                &mut stack,
                ResetStage::Slide,
                (amount > 0.0).then_some(NoteTransform::Slide { amount }),
            );
        }
        if let Some(semitones) = patch.transpose {
            replace(
                &mut stack,
                ResetStage::Transpose,
                (semitones != 0).then_some(NoteTransform::Transpose { semitones }),
            );
        }
        if let Some(ticks) = patch.lean_ticks {
            replace(
                &mut stack,
                ResetStage::Lean,
                (ticks != 0).then_some(NoteTransform::Lean { ticks }),
            );
        }
        if let Some(update) = &patch.strum {
            let settings = merge(strum(&stack), update)?;
            replace(
                &mut stack,
                ResetStage::Strum,
                Some(NoteTransform::Strum { settings }),
            );
        }
        if patch.brush.is_some() || patch.ghost.is_some() {
            let mut settings = ghost(&stack, clip.0);
            if let Some(brush) = patch.brush {
                settings.velocity = 0.3 * brush;
            }
            if let Some(update) = &patch.ghost {
                settings = merge(settings, update)?;
                if update.clear_target_pitch == Some(true) {
                    settings.target_pitch = None;
                }
            }
            replace(
                &mut stack,
                ResetStage::Ghost,
                Some(NoteTransform::Ghost { settings }),
            );
        }
        if let Some(update) = &patch.pitch {
            let settings = stack
                .iter()
                .find_map(|s| match s {
                    NoteTransform::Pitch { settings } => Some(settings.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let settings = merge(settings, update)?;
            replace(
                &mut stack,
                ResetStage::Pitch,
                Some(NoteTransform::Pitch { settings }),
            );
        }
        if patch.octave_above.is_some() || patch.octave_below.is_some() {
            let (above, below) = stack
                .iter()
                .find_map(|s| match s {
                    NoteTransform::Octaves { above, below } => Some((*above, *below)),
                    _ => None,
                })
                .unwrap_or((0.0, 0.0));
            let above = patch.octave_above.unwrap_or(above);
            let below = patch.octave_below.unwrap_or(below);
            replace(
                &mut stack,
                ResetStage::Octaves,
                (above > 0.0 || below > 0.0).then_some(NoteTransform::Octaves { above, below }),
            );
        }
        if let Some(update) = &patch.groove {
            let held = stack.iter().find_map(|s| match s {
                NoteTransform::Groove {
                    template,
                    timing,
                    velocity,
                } => Some((template.clone(), *timing, *velocity)),
                _ => None,
            });
            let template = if let Some(source) = update.reference_clip {
                self.clip_groove_template(ClipId(source))
                    .map_err(|e| e.to_string())?
            } else {
                held.as_ref()
                    .map(|h| h.0.clone())
                    .ok_or("Supply groove.reference_clip before changing groove strengths")?
            };
            let timing = update.timing.unwrap_or(held.as_ref().map_or(1.0, |h| h.1));
            let velocity = update
                .velocity
                .unwrap_or(held.as_ref().map_or(1.0, |h| h.2));
            replace(
                &mut stack,
                ResetStage::Groove,
                Some(NoteTransform::Groove {
                    template,
                    timing,
                    velocity,
                }),
            );
        }
        let mut outer = self
            .clip_transforms(clip)
            .map_err(|e| e.to_string())?
            .to_vec();
        if let Some(voice) = voice {
            let at = outer.iter().position(
                |s| matches!(s,NoteTransform::ForDrumVoice{voice:held,..} if held==voice),
            );
            match (at, stack.is_empty()) {
                (Some(at), _) => {
                    outer[at] = NoteTransform::ForDrumVoice {
                        voice: voice.into(),
                        transforms: stack,
                    }
                }
                (None, false) => {
                    let at = outer
                        .iter()
                        .position(|s| rank(s) > 7)
                        .unwrap_or(outer.len());
                    outer.insert(
                        at,
                        NoteTransform::ForDrumVoice {
                            voice: voice.into(),
                            transforms: stack,
                        },
                    );
                }
                (None, true) => {}
            }
        } else {
            outer = stack;
        }
        Ok(outer)
    }
}

/// Edits selected controls without rewriting a clip's stored notes.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PerformancePatch {
    /// Remove selected stage families before applying the supplied edits.
    #[schemars(length(max = 13))]
    pub reset: Vec<ResetStage>,
    /// Combined timing/velocity wander, 0..1; individual expression fields override it.
    #[schemars(range(min = 0, max = 1))]
    pub humanize: Option<f32>,
    /// Offbeat position, 50..75 percent; 50 is straight.
    #[schemars(range(min = 50, max = 75))]
    pub swing_percent: Option<u8>,
    /// Swing subdivision.
    #[schemars(with = "Option<String>", extend("enum" = ["eighth","sixteenth","eighth-triplet","sixteenth-triplet",null]))]
    pub swing_grid: Option<Subdivision>,
    /// Held fraction of written note lengths, 0.05..1.
    #[schemars(range(min = 0.05, max = 1))]
    pub gate: Option<f32>,
    /// Ticks added to every release, like a held sustain pedal; 960 ticks is one quarter note
    /// and 0 removes it. A note stops where the same pitch is struck again and at the clip end;
    /// attacks do not move and no notes are added.
    #[schemars(range(min = 0, max = 3840))]
    pub sustain_ticks: Option<i64>,
    /// Muted retrigger strength, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub mute: Option<f32>,
    /// Ghost velocity at the panel's scale, 0..1 maps to 0..0.3 relative gain.
    #[schemars(range(min = 0, max = 1))]
    pub brush: Option<f32>,
    /// Discrete intermediate-note strength, 0..1. This inserts a short performed note between
    /// eligible monophonic attacks; it is not a continuous pitch bend. Inspect `note_effects`
    /// for the inserted-note count and ineligible transition reasons.
    #[schemars(range(min = 0, max = 1))]
    pub slide: Option<f32>,
    /// Non-destructive transposition in semitones.
    #[schemars(range(min = -127, max = 127))]
    pub transpose: Option<i32>,
    /// Fixed timing offset in ticks; 960 ticks is one quarter note.
    #[schemars(range(min = -960, max = 960))]
    pub lean_ticks: Option<i64>,
    /// Relative velocity of upper-octave copies, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub octave_above: Option<f32>,
    /// Relative velocity of lower-octave copies, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub octave_below: Option<f32>,
    /// Independent timing, dynamics and ensemble settings.
    pub expression: Option<ExpressionPatch>,
    /// Chord-stroke settings.
    pub strum: Option<StrumPatch>,
    /// Ghost-note settings.
    pub ghost: Option<GhostPatch>,
    /// Automatic pitch/controller gestures for melodic instrument tracks.
    pub pitch: Option<PitchPatch>,
    /// Reference groove snapshot and strengths.
    pub groove: Option<GroovePatch>,
}

/// Independent expression edits; omitted fields preserve their current values.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ExpressionPatch {
    /// Timing wander, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub timing: Option<f32>,
    /// Velocity wander, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub velocity: Option<f32>,
    /// Phrase dynamics arch, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub swell: Option<f32>,
    /// Beat emphasis; negative emphasizes offbeats.
    #[schemars(range(min = -1, max = 1))]
    pub accent: Option<f32>,
    /// Ensemble motion blend, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub shared: Option<f32>,
    /// Ensemble group identity.
    pub group: Option<u64>,
    /// Push (negative) or delay (positive) in milliseconds.
    #[schemars(range(min = -50, max = 50))]
    pub delay_ms: Option<f32>,
    /// Private random take identity.
    pub seed: Option<u64>,
}

/// Chord-stroke edits; omitted fields preserve their current values.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct StrumPatch {
    /// Time between first and last struck pitches in milliseconds.
    #[schemars(range(min = 0, max = 100))]
    pub spread_ms: Option<f32>,
    /// Stroke direction.
    #[schemars(with = "Option<String>", extend("enum" = ["low_to_high","high_to_low","alternate",null]))]
    pub direction: Option<StrokeDirection>,
    /// Alternating-hand clock.
    #[schemars(with = "Option<String>", extend("enum" = ["attacks","eighths","sixteenths",null]))]
    pub clock: Option<StrumClock>,
    /// Highest notes used on an upstroke; zero uses the whole chord.
    #[schemars(range(min = 0, max = 4))]
    pub up_notes: Option<u8>,
    /// Upstroke relative velocity, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub up_velocity: Option<f32>,
    /// Lower-pitch downstroke emphasis, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub low_accent: Option<f32>,
}

/// Ghost-note edits; omitted fields preserve their current values.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct GhostPatch {
    /// Placement probability, independent of loudness.
    #[schemars(range(min = 0, max = 1))]
    pub density: Option<f32>,
    /// Relative ghost-note velocity, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub velocity: Option<f32>,
    /// Ghost length in milliseconds.
    #[schemars(range(min = 1, max = 100))]
    pub length_ms: Option<f32>,
    /// Candidate placement pattern.
    #[schemars(with = "Option<String>", extend("enum" = ["sixteenths","offbeats","pickup","repeating",null]))]
    pub pattern: Option<GhostPattern>,
    /// Variation between bar and loop repetitions.
    #[schemars(range(min = 0, max = 1))]
    pub variation: Option<f32>,
    /// Preserve long gaps and the phrase tail.
    pub preserve_rests: Option<bool>,
    /// Largest gap to fill, in quarter-note beats.
    #[schemars(range(min = 0, max = 64))]
    pub max_gap_beats: Option<f32>,
    /// Restrict ghosts to one existing MIDI pitch.
    #[schemars(range(min = 0, max = 127))]
    pub target_pitch: Option<u8>,
    /// Set true to return to chord ghosts; omit target_pitch when clearing.
    pub clear_target_pitch: Option<bool>,
    /// Stored random take identity.
    pub seed: Option<u64>,
}

/// Automatic pitch/controller edits; existing authored curves and contour shapes are retained.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PitchPatch {
    /// Scoop depth in semitones.
    #[schemars(range(min = 0, max = 4))]
    pub scoop: Option<f32>,
    /// Scoop duration in milliseconds.
    #[schemars(range(min = 10, max = 300))]
    pub scoop_ms: Option<f32>,
    /// Vibrato depth in semitones.
    #[schemars(range(min = 0, max = 1))]
    pub vibrato: Option<f32>,
    /// Vibrato frequency in Hz.
    #[schemars(range(min = 2, max = 9))]
    pub vibrato_hz: Option<f32>,
    /// Vibrato onset delay in milliseconds.
    #[schemars(range(min = 0, max = 1000))]
    pub vibrato_delay_ms: Option<f32>,
    /// Release fall depth in semitones.
    #[schemars(range(min = 0, max = 12))]
    pub fall: Option<f32>,
    /// Release fall duration in milliseconds.
    #[schemars(range(min = 10, max = 500))]
    pub fall_ms: Option<f32>,
    /// Connection duration on each side of a qualifying melodic note boundary. This creates a
    /// continuous pitch bend and is distinct from `slide`, which inserts an intermediate note.
    /// Connections require a monophonic performed line, a gap of at most 60 ms, an interval of at
    /// most two octaves, and notes at least 80 ms long. Inspect `note_effects` for curve ablation
    /// (`glide_ms` enabled versus disabled); a stored setting alone is not evidence of effect.
    #[schemars(range(min = 0, max = 250))]
    pub glide_ms: Option<f32>,
    /// Modulation wheel depth, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub modulation: Option<f32>,
    /// Long-note volume contour strength, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub volume_swell: Option<f32>,
}

/// Reference groove edits; capture is a snapshot, not a live link.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct GroovePatch {
    /// Capture the first performed pass of this MIDI clip on a sixteenth-note grid.
    pub reference_clip: Option<u64>,
    /// Timing transfer strength, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub timing: Option<f32>,
    /// Relative dynamics transfer strength, 0..1.
    #[schemars(range(min = 0, max = 1))]
    pub velocity: Option<f32>,
}

/// A family of performance stages to remove explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResetStage {
    /// Remove the swing family, including its legacy representation.
    Swing,
    /// Remove the gate family, including its legacy representation.
    Gate,
    /// Remove the sustain (held release) stage.
    Sustain,
    /// Remove the expression family, including its legacy representation.
    Expression,
    /// Remove the strum family, including its legacy representation.
    Strum,
    /// Remove the ghost family, including its legacy representation.
    Ghost,
    /// Remove the mute family, including its legacy representation.
    Mute,
    /// Remove the slide family, including its legacy representation.
    Slide,
    /// Remove the pitch family, including its legacy representation.
    Pitch,
    /// Remove the octaves family, including its legacy representation.
    Octaves,
    /// Remove the groove family, including its legacy representation.
    Groove,
    /// Remove the transpose family, including its legacy representation.
    Transpose,
    /// Remove the lean family, including its legacy representation.
    Lean,
}

fn bounded(name: &str, value: f64, min: f64, max: f64) -> Result<(), String> {
    if !value.is_finite() || value < min || value > max {
        return Err(format!("{name} must be finite and within {min}..{max}"));
    }
    Ok(())
}

impl PerformancePatch {
    fn validate(&self) -> Result<(), String> {
        if self.reset.len() > 13 {
            return Err("reset accepts at most 13 stage families".into());
        }
        let fields = serde_json::to_value(self).map_err(|e| e.to_string())?;
        for name in ["expression", "strum", "ghost", "pitch", "groove"] {
            if !fields[name].is_null() && !has_update(&fields[name]) {
                return Err(format!("Supply at least one {name} setting or omit {name}"));
            }
        }
        if let Some(value) = self.humanize {
            bounded("PerformancePatch.humanize", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.swing_percent {
            bounded("PerformancePatch.swing_percent", value as f64, 50.0, 75.0)?;
        }
        if let Some(value) = self.gate {
            bounded("PerformancePatch.gate", value as f64, 0.05, 1.0)?;
        }
        if let Some(value) = self.sustain_ticks {
            bounded("PerformancePatch.sustain_ticks", value as f64, 0.0, 3840.0)?;
        }
        if let Some(value) = self.mute {
            bounded("PerformancePatch.mute", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.brush {
            bounded("PerformancePatch.brush", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.slide {
            bounded("PerformancePatch.slide", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.transpose {
            bounded("PerformancePatch.transpose", value as f64, -127.0, 127.0)?;
        }
        if let Some(value) = self.lean_ticks {
            bounded("PerformancePatch.lean_ticks", value as f64, -960.0, 960.0)?;
        }
        if let Some(value) = self.octave_above {
            bounded("PerformancePatch.octave_above", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.octave_below {
            bounded("PerformancePatch.octave_below", value as f64, 0.0, 1.0)?;
        }
        if let Some(patch) = &self.expression {
            patch.validate()?;
        }
        if let Some(patch) = &self.strum {
            patch.validate()?;
        }
        if let Some(patch) = &self.ghost {
            patch.validate()?;
        }
        if let Some(patch) = &self.pitch {
            patch.validate()?;
        }
        if let Some(patch) = &self.groove {
            patch.validate()?;
        }
        Ok(())
    }
}

impl ExpressionPatch {
    fn validate(&self) -> Result<(), String> {
        if let Some(value) = self.timing {
            bounded("ExpressionPatch.timing", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.velocity {
            bounded("ExpressionPatch.velocity", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.swell {
            bounded("ExpressionPatch.swell", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.accent {
            bounded("ExpressionPatch.accent", value as f64, -1.0, 1.0)?;
        }
        if let Some(value) = self.shared {
            bounded("ExpressionPatch.shared", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.delay_ms {
            bounded("ExpressionPatch.delay_ms", value as f64, -50.0, 50.0)?;
        }
        Ok(())
    }
}

impl StrumPatch {
    fn validate(&self) -> Result<(), String> {
        if let Some(value) = self.spread_ms {
            bounded("StrumPatch.spread_ms", value as f64, 0.0, 100.0)?;
        }
        if let Some(value) = self.up_notes {
            bounded("StrumPatch.up_notes", value as f64, 0.0, 4.0)?;
        }
        if let Some(value) = self.up_velocity {
            bounded("StrumPatch.up_velocity", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.low_accent {
            bounded("StrumPatch.low_accent", value as f64, 0.0, 1.0)?;
        }
        Ok(())
    }
}

impl GhostPatch {
    fn validate(&self) -> Result<(), String> {
        if let Some(value) = self.density {
            bounded("GhostPatch.density", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.velocity {
            bounded("GhostPatch.velocity", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.length_ms {
            bounded("GhostPatch.length_ms", value as f64, 1.0, 100.0)?;
        }
        if let Some(value) = self.variation {
            bounded("GhostPatch.variation", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.max_gap_beats {
            bounded("GhostPatch.max_gap_beats", value as f64, 0.0, 64.0)?;
        }
        if let Some(value) = self.target_pitch {
            bounded("GhostPatch.target_pitch", value as f64, 0.0, 127.0)?;
        }
        if self.clear_target_pitch == Some(true) && self.target_pitch.is_some() {
            return Err("Omit target_pitch when clear_target_pitch is true".into());
        }
        Ok(())
    }
}

impl PitchPatch {
    fn validate(&self) -> Result<(), String> {
        if let Some(value) = self.scoop {
            bounded("PitchPatch.scoop", value as f64, 0.0, 4.0)?;
        }
        if let Some(value) = self.scoop_ms {
            bounded("PitchPatch.scoop_ms", value as f64, 10.0, 300.0)?;
        }
        if let Some(value) = self.vibrato {
            bounded("PitchPatch.vibrato", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.vibrato_hz {
            bounded("PitchPatch.vibrato_hz", value as f64, 2.0, 9.0)?;
        }
        if let Some(value) = self.vibrato_delay_ms {
            bounded("PitchPatch.vibrato_delay_ms", value as f64, 0.0, 1000.0)?;
        }
        if let Some(value) = self.fall {
            bounded("PitchPatch.fall", value as f64, 0.0, 12.0)?;
        }
        if let Some(value) = self.fall_ms {
            bounded("PitchPatch.fall_ms", value as f64, 10.0, 500.0)?;
        }
        if let Some(value) = self.glide_ms {
            bounded("PitchPatch.glide_ms", value as f64, 0.0, 250.0)?;
        }
        if let Some(value) = self.modulation {
            bounded("PitchPatch.modulation", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.volume_swell {
            bounded("PitchPatch.volume_swell", value as f64, 0.0, 1.0)?;
        }
        Ok(())
    }
}

impl GroovePatch {
    fn validate(&self) -> Result<(), String> {
        if let Some(value) = self.timing {
            bounded("GroovePatch.timing", value as f64, 0.0, 1.0)?;
        }
        if let Some(value) = self.velocity {
            bounded("GroovePatch.velocity", value as f64, 0.0, 1.0)?;
        }
        Ok(())
    }
}
