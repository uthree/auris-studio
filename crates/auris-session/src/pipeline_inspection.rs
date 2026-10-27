//! Bounded, read-only inspection of intermediate pipeline state.
//!
//! This module inspects document data and note transforms only. It never creates a plugin or
//! renders audio, so passive graph reads remain safe while a project is partially installed.

use auris_core::{
    ClipId, Note, NoteTransform, ParamTarget, ParamUnit, PerformanceContext, SendId, Ticks,
    TrackId, performed_notes,
};
use serde::{Deserialize, Serialize};

use crate::pipeline_graph::PipelineNodeData;
use crate::{Session, SessionError};

/// A bounded request for one clip stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineInspectionRequest {
    /// Clip owning the stage stack.
    pub clip: ClipId,
    /// Zero-based stage ordinal.
    pub stage_index: usize,
    /// Timeline position associated with this read. Stage previews use pass zero.
    pub at_tick: Ticks,
    /// Maximum sampled notes returned on each side of the stage.
    pub max_notes: usize,
}

impl PipelineInspectionRequest {
    /// Maximum sample size accepted at the session boundary.
    pub const MAX_NOTES: usize = 256;
    /// Maximum authored notes inspected in one pass.
    pub const MAX_SOURCE_NOTES: usize = 4096;
    /// Maximum stage prefix evaluated by one request.
    pub const MAX_STAGES: usize = 64;
    /// Maximum nested transform records in a stage prefix.
    pub const MAX_NESTED_TRANSFORMS: usize = 256;
    /// Maximum performed notes materialized by a passive one-stage read.
    pub const MAX_PERFORMED_NOTES: usize = 32_768;
    /// Maximum nesting depth traversed in a saved transform tree.
    pub const MAX_NESTING_DEPTH: usize = 64;

    fn bounded_max_notes(self) -> usize {
        self.max_notes.clamp(1, Self::MAX_NOTES)
    }
}

/// Why a passive inspection is complete or bounded before evaluation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum PipelineInspectionStatus {
    /// Input and output were evaluated using the stored transform order.
    Complete,
    /// Evaluation was intentionally skipped because its work estimate exceeded the read budget.
    Limited {
        /// Human-readable bound that prevented evaluation.
        reason: String,
    },
}

/// One compact note in an intermediate-stage sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipelineNoteSample {
    /// MIDI pitch.
    pub pitch: u8,
    /// Linear velocity.
    pub velocity: f32,
    /// Relative attack position in ticks.
    pub start: Ticks,
    /// Performed duration in ticks.
    pub length: Ticks,
    /// Drum writer, when present.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub drum_voice: String,
}

impl From<&Note> for PipelineNoteSample {
    fn from(note: &Note) -> Self {
        Self {
            pitch: note.pitch,
            velocity: note.velocity,
            start: note.start,
            length: note.length,
            drum_voice: note.drum_voice.clone(),
        }
    }
}

fn contains_deferred_control(transform: &NoteTransform) -> bool {
    match transform {
        NoteTransform::Pitch { .. } => true,
        NoteTransform::ForDrumVoice { transforms, .. } => {
            transforms.iter().any(contains_deferred_control)
        }
        _ => false,
    }
}

fn nested_transform_count(transforms: &[NoteTransform], depth: usize) -> Option<usize> {
    if depth >= PipelineInspectionRequest::MAX_NESTING_DEPTH {
        return None;
    }
    let mut count = 0usize;
    for transform in transforms {
        count = count.checked_add(1)?;
        if let NoteTransform::ForDrumVoice { transforms, .. } = transform {
            count = count.checked_add(nested_transform_count(transforms, depth + 1)?)?;
        }
        if count > PipelineInspectionRequest::MAX_NESTED_TRANSFORMS {
            return Some(count);
        }
    }
    Some(count)
}

/// Whether a stage output is note data or deferred controller data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStageState {
    /// The stage changes note data in the inspected pass.
    Notes,
    /// The stage emits controller events and does not linearly alter note data.
    DeferredControl,
}

/// A bounded description of one stage boundary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipelineIntermediateState {
    /// Clip inspected.
    pub clip: ClipId,
    /// Stage inspected.
    pub stage_index: usize,
    /// Tick captured by the caller; stage evaluation itself is pass zero.
    pub at_tick: Ticks,
    /// Stored transform discriminant.
    pub transform_kind: String,
    /// Stored transform settings.
    pub settings: serde_json::Value,
    /// Note or deferred-controller semantics.
    pub state: PipelineStageState,
    /// Whether evaluation completed within the passive-read budget.
    pub availability: PipelineInspectionStatus,
    /// Number of notes entering this stage, when evaluated.
    pub input_count: usize,
    /// Number of notes leaving this stage, when evaluated.
    pub output_count: usize,
    /// Bounded input sample.
    pub input: Vec<PipelineNoteSample>,
    /// Bounded output sample.
    pub output: Vec<PipelineNoteSample>,
    /// Whether either sample was capped.
    pub truncated: bool,
}

/// One parameter value in a node detail snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipelineParameterState {
    /// Stored parameter target.
    pub target: ParamTarget,
    /// Stored plugin parameter key, when applicable.
    pub key: Option<String>,
    /// Display unit, when known.
    pub unit: Option<ParamUnit>,
    /// Static document value.
    pub static_value: Option<f32>,
    /// Automation curve value at the captured tick.
    pub automation_value: Option<f32>,
    /// Value effective at the captured tick.
    pub effective_value: Option<f32>,
    /// Whether the owner/parameter binding was checked without loading a plugin.
    pub binding_checked: bool,
}

/// Detail for a stable pipeline graph node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PipelineInspectionDetail {
    /// One clip transform stage.
    Stage {
        /// Intermediate stage state.
        state: PipelineIntermediateState,
    },
    /// One or more parameter values associated with a node.
    Parameters {
        /// Parameter snapshots associated with the node.
        values: Vec<PipelineParameterState>,
    },
    /// Known node whose detail is intentionally not available.
    Unavailable {
        /// Why no passive detail is available.
        reason: String,
    },
}

/// Detail snapshot for a stable `pipeline_read` node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipelineNodeInspection {
    /// Stable graph node address.
    pub node: String,
    /// Absolute project tick used for automation evaluation.
    pub at_tick: Ticks,
    /// Node-specific passive detail.
    pub detail: PipelineInspectionDetail,
}

fn limited_stage(
    clip: ClipId,
    stage_index: usize,
    at_tick: Ticks,
    kind: String,
    settings: serde_json::Value,
    state: PipelineStageState,
    reason: String,
) -> PipelineIntermediateState {
    PipelineIntermediateState {
        clip,
        stage_index,
        at_tick,
        transform_kind: kind,
        settings,
        state,
        availability: PipelineInspectionStatus::Limited { reason },
        input_count: 0,
        output_count: 0,
        input: Vec::new(),
        output: Vec::new(),
        truncated: false,
    }
}

impl Session {
    /// Inspects one clip stage without changing history, graph, or plugin state.
    pub fn inspect_pipeline_stage(
        &self,
        request: PipelineInspectionRequest,
    ) -> Result<PipelineIntermediateState, SessionError> {
        if request.at_tick < Ticks::ZERO {
            return Err(SessionError::InvalidNotes(
                "pipeline inspection tick must not be negative".into(),
            ));
        }
        let Some((_, clip)) = self.project().midi_clip(request.clip) else {
            return Err(SessionError::UnknownClip(request.clip.0));
        };
        let transform = clip.transforms.get(request.stage_index).ok_or_else(|| {
            SessionError::InvalidNotes(format!(
                "pipeline stage index {} is out of range",
                request.stage_index
            ))
        })?;
        let prefix = request.stage_index + 1;
        if prefix > PipelineInspectionRequest::MAX_STAGES {
            return Ok(limited_stage(
                request.clip,
                request.stage_index,
                request.at_tick,
                "transform".into(),
                serde_json::Value::Null,
                PipelineStageState::Notes,
                format!(
                    "stage prefix exceeds {} stages",
                    PipelineInspectionRequest::MAX_STAGES
                ),
            ));
        }
        let Some(nested_count) = nested_transform_count(&clip.transforms[..prefix], 0) else {
            return Ok(limited_stage(
                request.clip,
                request.stage_index,
                request.at_tick,
                "transform".into(),
                serde_json::Value::Null,
                PipelineStageState::Notes,
                format!(
                    "transform nesting exceeds {} levels",
                    PipelineInspectionRequest::MAX_NESTING_DEPTH
                ),
            ));
        };
        if nested_count > PipelineInspectionRequest::MAX_NESTED_TRANSFORMS {
            return Ok(limited_stage(
                request.clip,
                request.stage_index,
                request.at_tick,
                "transform".into(),
                serde_json::Value::Null,
                PipelineStageState::Notes,
                format!(
                    "nested transform count exceeds {}",
                    PipelineInspectionRequest::MAX_NESTED_TRANSFORMS
                ),
            ));
        }
        let settings = serde_json::to_value(transform).map_err(|error| {
            SessionError::InvalidNotes(format!("pipeline stage settings: {error}"))
        })?;
        let kind = settings["kind"].as_str().unwrap_or("transform").to_owned();
        let state = if contains_deferred_control(transform) {
            PipelineStageState::DeferredControl
        } else {
            PipelineStageState::Notes
        };
        if clip.notes.len() > PipelineInspectionRequest::MAX_SOURCE_NOTES {
            return Ok(limited_stage(
                request.clip,
                request.stage_index,
                request.at_tick,
                kind,
                settings,
                state,
                format!(
                    "authored note count exceeds {}",
                    PipelineInspectionRequest::MAX_SOURCE_NOTES
                ),
            ));
        }
        let authored: Vec<_> = clip.playable_notes().collect();
        let authored_count = authored.len();
        // Estimate only the first pass. Controllers and later loop passes are not evaluated.
        let mut prefix_clip =
            auris_core::MidiClip::new(clip.id, "inspection", clip.start, clip.length);
        prefix_clip.notes = authored.clone();
        prefix_clip.transforms = clip.transforms[..prefix].to_vec();
        let output_bound = match prefix_clip.looped_note_instances() {
            Err(error) => {
                return Ok(limited_stage(
                    request.clip,
                    request.stage_index,
                    request.at_tick,
                    kind,
                    settings,
                    state,
                    format!("performed-note workload unavailable: {error}"),
                ));
            }
            Ok(count) => count,
        };
        let input_bound = if request.stage_index > 0 {
            prefix_clip.transforms.truncate(request.stage_index);
            match prefix_clip.looped_note_instances() {
                Err(error) => {
                    return Ok(limited_stage(
                        request.clip,
                        request.stage_index,
                        request.at_tick,
                        kind,
                        settings,
                        state,
                        format!("input workload unavailable: {error}"),
                    ));
                }
                Ok(count) => count,
            }
        } else {
            authored_count
        };
        if output_bound > PipelineInspectionRequest::MAX_PERFORMED_NOTES
            || input_bound > PipelineInspectionRequest::MAX_PERFORMED_NOTES
            || output_bound.saturating_add(input_bound)
                > PipelineInspectionRequest::MAX_PERFORMED_NOTES
        {
            return Ok(limited_stage(
                request.clip,
                request.stage_index,
                request.at_tick,
                kind,
                settings,
                state,
                format!(
                    "combined performed-note workload exceeds {}",
                    PipelineInspectionRequest::MAX_PERFORMED_NOTES
                ),
            ));
        }
        // Evaluate prefixes separately: stage N input is exactly the output of N-1.
        let context = || PerformanceContext {
            bpm: self.project().tempo_map.bpm_at(clip.start),
            pass: 0,
            start: clip.start,
            length: clip.length,
            signatures: &self.project().signatures,
        };
        let input = if request.stage_index == 0 {
            authored.clone()
        } else {
            performed_notes(
                authored.clone(),
                &clip.transforms[..request.stage_index],
                context(),
            )
        };
        let output = performed_notes(
            authored,
            &clip.transforms[..=request.stage_index],
            context(),
        );
        let max_notes = request.bounded_max_notes();
        Ok(PipelineIntermediateState {
            clip: request.clip,
            stage_index: request.stage_index,
            at_tick: request.at_tick,
            transform_kind: kind,
            settings,
            state,
            availability: PipelineInspectionStatus::Complete,
            input_count: input.len(),
            output_count: output.len(),
            input: input
                .iter()
                .take(max_notes)
                .map(PipelineNoteSample::from)
                .collect(),
            output: output
                .iter()
                .take(max_notes)
                .map(PipelineNoteSample::from)
                .collect(),
            truncated: input.len() > max_notes || output.len() > max_notes,
        })
    }

    /// Inspects a stable `pipeline_read` node without loading runtime plugins.
    pub fn inspect_pipeline_node(
        &self,
        node: &str,
        at_tick: Ticks,
    ) -> Result<PipelineNodeInspection, SessionError> {
        if at_tick < Ticks::ZERO {
            return Err(SessionError::InvalidNotes(
                "pipeline inspection tick must not be negative".into(),
            ));
        }
        if let Some((clip, stage)) = node
            .strip_prefix("clip:")
            .and_then(|rest| rest.split_once("/stage:"))
            .and_then(|(clip, stage)| Some((clip.parse().ok()?, stage.parse().ok()?)))
            .filter(|(clip, stage)| format!("clip:{clip}/stage:{stage}") == node)
        {
            let state = self.inspect_pipeline_stage(PipelineInspectionRequest {
                clip: ClipId(clip),
                stage_index: stage,
                at_tick,
                max_notes: 32,
            })?;
            return Ok(PipelineNodeInspection {
                node: node.to_owned(),
                at_tick,
                detail: PipelineInspectionDetail::Stage { state },
            });
        }
        if node.starts_with("clip:") && node.contains("/stage:") {
            return Ok(PipelineNodeInspection {
                node: node.to_owned(),
                at_tick,
                detail: PipelineInspectionDetail::Unavailable {
                    reason: "stage node address is not canonical".into(),
                },
            });
        }
        let Some(found) = self
            .pipeline_node(node)
            .map_err(|error| SessionError::InvalidNotes(error.to_string()))?
        else {
            return Ok(PipelineNodeInspection {
                node: node.to_owned(),
                at_tick,
                detail: PipelineInspectionDetail::Unavailable {
                    reason: "node was not found in the current graph".into(),
                },
            });
        };
        match &found.data {
            PipelineNodeData::Automation {
                target,
                key,
                value_unit,
                parameter_checked,
                ..
            } => {
                let binding_checked = *parameter_checked;
                let static_value = binding_checked
                    .then(|| {
                        Self::mixer_descriptor(*target)
                            .map(|descriptor| self.param_value(*target, &descriptor))
                    })
                    .flatten();
                let curve_value = self.automated_value(*target, at_tick);
                Ok(PipelineNodeInspection {
                    node: node.to_owned(),
                    at_tick,
                    detail: PipelineInspectionDetail::Parameters {
                        values: vec![PipelineParameterState {
                            target: *target,
                            key: key.clone(),
                            unit: *value_unit,
                            static_value,
                            automation_value: curve_value,
                            effective_value: binding_checked
                                .then_some(curve_value.or(static_value))
                                .flatten(),
                            binding_checked,
                        }],
                    },
                })
            }
            PipelineNodeData::Fader { gain_db, pan, .. } => {
                let target = found.track_id.map_or(ParamTarget::MasterGain, |id| {
                    ParamTarget::TrackGain(TrackId(id))
                });
                let pan_target = found.track_id.map_or(ParamTarget::MasterPan, |id| {
                    ParamTarget::TrackPan(TrackId(id))
                });
                Ok(PipelineNodeInspection {
                    node: node.to_owned(),
                    at_tick,
                    detail: PipelineInspectionDetail::Parameters {
                        values: vec![
                            PipelineParameterState {
                                target,
                                key: None,
                                unit: Some(ParamUnit::Decibels),
                                static_value: Some(*gain_db),
                                automation_value: self.automated_value(target, at_tick),
                                effective_value: self
                                    .automated_value(target, at_tick)
                                    .or(Some(*gain_db)),
                                binding_checked: true,
                            },
                            PipelineParameterState {
                                target: pan_target,
                                key: None,
                                unit: Some(ParamUnit::Pan),
                                static_value: Some(*pan),
                                automation_value: self.automated_value(pan_target, at_tick),
                                effective_value: self
                                    .automated_value(pan_target, at_tick)
                                    .or(Some(*pan)),
                                binding_checked: true,
                            },
                        ],
                    },
                })
            }
            PipelineNodeData::Send { send, level_db, .. } => {
                let Some(track) = found.track_id else {
                    return Ok(PipelineNodeInspection {
                        node: node.to_owned(),
                        at_tick,
                        detail: PipelineInspectionDetail::Unavailable {
                            reason: "send has no owning track".into(),
                        },
                    });
                };
                let target = ParamTarget::Send {
                    track: TrackId(track),
                    send: SendId(*send),
                };
                let automation_value = self.automated_value(target, at_tick);
                Ok(PipelineNodeInspection {
                    node: node.to_owned(),
                    at_tick,
                    detail: PipelineInspectionDetail::Parameters {
                        values: vec![PipelineParameterState {
                            target,
                            key: None,
                            unit: Some(ParamUnit::Decibels),
                            static_value: Some(*level_db),
                            automation_value,
                            effective_value: automation_value.or(Some(*level_db)),
                            binding_checked: true,
                        }],
                    },
                })
            }
            _ => Ok(PipelineNodeInspection {
                node: node.to_owned(),
                at_tick,
                detail: PipelineInspectionDetail::Unavailable {
                    reason: "this node has no passive intermediate detail".into(),
                },
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionOptions;
    use auris_core::{NoteTransform, PitchPerformance};

    fn session_with_clip() -> (Session, ClipId) {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks::QUARTER)])
            .unwrap();
        (session, clip)
    }

    #[test]
    fn stage_input_is_previous_prefix_output_in_order() {
        let (mut session, clip) = session_with_clip();
        session
            .set_clip_transforms(
                clip,
                vec![
                    NoteTransform::Transpose { semitones: 12 },
                    NoteTransform::Transpose { semitones: -5 },
                ],
            )
            .unwrap();
        let state = session
            .inspect_pipeline_stage(PipelineInspectionRequest {
                clip,
                stage_index: 1,
                at_tick: Ticks::ZERO,
                max_notes: 16,
            })
            .unwrap();
        assert_eq!(state.input[0].pitch, 72);
        assert_eq!(state.output[0].pitch, 67);
        assert_eq!(state.input_count, 1);
        assert_eq!(state.output_count, 1);
    }

    #[test]
    fn deferred_pitch_is_never_presented_as_linear_note_execution() {
        let (mut session, clip) = session_with_clip();
        session
            .set_clip_transforms(
                clip,
                vec![
                    NoteTransform::Transpose { semitones: 12 },
                    NoteTransform::Pitch {
                        settings: PitchPerformance::default(),
                    },
                ],
            )
            .unwrap();
        let state = session
            .inspect_pipeline_stage(PipelineInspectionRequest {
                clip,
                stage_index: 1,
                at_tick: Ticks::ZERO,
                max_notes: 16,
            })
            .unwrap();
        assert_eq!(state.state, PipelineStageState::DeferredControl);
        assert_eq!(state.input[0].pitch, 72);
        assert_eq!(state.output[0].pitch, 72);
    }

    #[test]
    fn workload_limit_returns_limited_without_truncating_source() {
        let (mut session, clip) = session_with_clip();
        let mut notes = Vec::new();
        for pitch in 0..=127 {
            for index in 0..33 {
                notes.push(Note::new(pitch, Ticks(index * 10), Ticks::QUARTER));
            }
        }
        session.replace_notes(clip, notes).unwrap();
        session
            .set_clip_transforms(clip, vec![NoteTransform::Transpose { semitones: 1 }])
            .unwrap();
        let state = session
            .inspect_pipeline_stage(PipelineInspectionRequest {
                clip,
                stage_index: 0,
                at_tick: Ticks::ZERO,
                max_notes: 4,
            })
            .unwrap();
        assert!(matches!(
            state.availability,
            PipelineInspectionStatus::Limited { .. }
        ));
        assert!(state.input.is_empty());
    }

    #[test]
    fn sample_cap_marks_complete_state_as_truncated() {
        let (mut session, clip) = session_with_clip();
        let notes = (0..40)
            .map(|index| Note::new(60, Ticks(index * 10), Ticks::QUARTER))
            .collect();
        session.replace_notes(clip, notes).unwrap();
        session
            .set_clip_transforms(clip, vec![NoteTransform::Transpose { semitones: 1 }])
            .unwrap();
        let state = session
            .inspect_pipeline_stage(PipelineInspectionRequest {
                clip,
                stage_index: 0,
                at_tick: Ticks::ZERO,
                max_notes: 32,
            })
            .unwrap();
        assert!(matches!(
            state.availability,
            PipelineInspectionStatus::Complete
        ));
        assert_eq!(state.input.len(), 32);
        assert!(state.truncated);
    }

    #[test]
    fn nesting_limit_is_checked_before_settings_serialization() {
        let (mut session, clip) = session_with_clip();
        let mut nested = NoteTransform::Transpose { semitones: 1 };
        for _ in 0..=PipelineInspectionRequest::MAX_NESTING_DEPTH {
            nested = NoteTransform::ForDrumVoice {
                voice: "kit".into(),
                transforms: vec![nested],
            };
        }
        session.set_clip_transforms(clip, vec![nested]).unwrap();
        let state = session
            .inspect_pipeline_stage(PipelineInspectionRequest {
                clip,
                stage_index: 0,
                at_tick: Ticks::ZERO,
                max_notes: 4,
            })
            .unwrap();
        assert!(matches!(
            state.availability,
            PipelineInspectionStatus::Limited { .. }
        ));
        assert!(state.settings.is_null());
    }

    #[test]
    fn expansion_workload_limit_is_reported_before_materializing_output() {
        let (mut session, clip) = session_with_clip();
        let notes = (0..1024)
            .map(|index| Note::new(60, Ticks(index), Ticks::QUARTER))
            .collect();
        session.replace_notes(clip, notes).unwrap();
        session
            .set_clip_transforms(
                clip,
                vec![
                    NoteTransform::Octaves {
                        above: 1.0,
                        below: 1.0,
                    },
                    NoteTransform::Octaves {
                        above: 1.0,
                        below: 1.0,
                    },
                    NoteTransform::Octaves {
                        above: 1.0,
                        below: 1.0,
                    },
                    NoteTransform::Octaves {
                        above: 1.0,
                        below: 1.0,
                    },
                ],
            )
            .unwrap();
        let state = session
            .inspect_pipeline_stage(PipelineInspectionRequest {
                clip,
                stage_index: 3,
                at_tick: Ticks::ZERO,
                max_notes: 4,
            })
            .unwrap();
        assert!(matches!(
            state.availability,
            PipelineInspectionStatus::Limited { .. }
        ));
        assert!(state.output.is_empty());
    }

    #[test]
    fn node_automation_reports_curve_and_keeps_session_read_only() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        assert!(session.set_automation_point(ParamTarget::TrackGain(track), Ticks::ZERO, -12.0));
        assert!(session.set_automation_point(
            ParamTarget::TrackGain(track),
            Ticks::QUARTER * 4,
            0.0
        ));
        let revision = session.revision();
        let node = format!("automation:track:{}/gain", track.0);
        let detail = session
            .inspect_pipeline_node(&node, Ticks::QUARTER * 2)
            .unwrap();
        assert_eq!(session.revision(), revision);
        assert_eq!(detail.at_tick, Ticks::QUARTER * 2);
        let PipelineInspectionDetail::Parameters { values } = detail.detail else {
            panic!("expected parameter detail");
        };
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].automation_value, Some(-6.0));
        assert_eq!(values[0].effective_value, Some(-6.0));
        assert!(values[0].binding_checked);
    }

    #[test]
    fn negative_inspection_tick_is_rejected() {
        let (session, clip) = session_with_clip();
        let error = session
            .inspect_pipeline_stage(PipelineInspectionRequest {
                clip,
                stage_index: 0,
                at_tick: Ticks(-1),
                max_notes: 4,
            })
            .unwrap_err();
        assert!(error.to_string().contains("must not be negative"));
    }

    #[test]
    fn stage_node_address_must_be_canonical() {
        let (session, _) = session_with_clip();
        let detail = session
            .inspect_pipeline_node("clip:+1/stage:0", Ticks::ZERO)
            .unwrap();
        assert!(matches!(
            detail.detail,
            PipelineInspectionDetail::Unavailable { .. }
        ));
    }

    #[test]
    fn node_detail_finds_a_node_beyond_the_public_page_limit() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let mut last = None;
        for index in 0..70 {
            last = Some(
                session
                    .add_default_instrument_track(format!("Track {index}"))
                    .unwrap(),
            );
        }
        let track = last.unwrap();
        let detail = session
            .inspect_pipeline_node(&format!("fader:track:{}", track.0), Ticks::ZERO)
            .unwrap();
        assert!(matches!(
            detail.detail,
            PipelineInspectionDetail::Parameters { .. }
        ));
    }
}
