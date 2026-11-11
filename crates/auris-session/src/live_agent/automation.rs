//! Exact, native-unit parameter automation shared by live and saved agents.

use crate::{ParamTarget, Session, prelude::*};
use auris_core::automation::{AutomationCurve, AutomationPoint};
use serde::{Deserialize, Serialize};

/// One stable parameter address, independent of names and effect positions.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlTarget {
    /// Track gain in decibels.
    TrackGain {
        /// Stable track ID.
        track: u64,
    },
    /// Track pan in normalized native units.
    TrackPan {
        /// Stable track ID.
        track: u64,
    },
    /// Master gain in decibels.
    MasterGain,
    /// Master pan in normalized native units.
    MasterPan,
    /// The level of one of a track's outputs, in decibels.
    Output {
        /// Stable source track node from mixer_graph, such as track:12.
        source: String,
        /// Stable destination bus node or track:master.
        target: String,
        /// Which output: true for the one taken before the fader. Default false.
        #[serde(default)]
        pre_fader: bool,
    },
    /// Instrument parameter discovered with inspect_instrument.
    Instrument {
        /// Stable instrument track ID.
        track: u64,
        /// Exact descriptor key.
        param: String,
    },
    /// Effect parameter discovered with mixer_graph parameters.
    Effect {
        /// Stable effect node ID.
        node: String,
        /// Exact descriptor key.
        param: String,
    },
}

/// Native automation interpolation.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// Linear ramps between points.
    Linear,
    /// Hold values until the next point; required for discrete controls.
    Hold,
}

/// A parameter value at an exact absolute timeline tick.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Point {
    /// Absolute nonnegative timeline tick.
    pub at_tick: i64,
    /// Value in the descriptor's native units.
    pub value: f32,
}

/// Read or atomically revise one parameter lane.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Read descriptor, static value, interpolation, points and choice labels.
    Read {
        /// Zero-based point page offset.
        #[serde(default)]
        offset: usize,
        /// Zero-based choice-label page offset.
        #[serde(default)]
        choice_offset: usize,
    },
    /// Merge or replace exact positioned values as one Undo step.
    Set {
        /// 1..4096 unique positions; values use descriptor units.
        points: Vec<Point>,
        /// Replace the entire lane instead of merging supplied positions.
        #[serde(default)]
        replace: bool,
        /// Optional interpolation; omitted preserves the existing/default shape.
        curve: Option<Interpolation>,
    },
    /// Clear this one lane, preserving the static value.
    Clear,
}

impl Operation {
    /// Whether this operation only reads the document.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Read { .. })
    }
}

impl Session {
    pub(crate) fn agent_control_target(
        &mut self,
        target: &ControlTarget,
    ) -> Result<ParamTarget, String> {
        let track_exists = |id| {
            self.project()
                .track(TrackId(id))
                .map(|_| TrackId(id))
                .ok_or("Unknown track ID")
        };
        match target {
            ControlTarget::TrackGain { track } => Ok(ParamTarget::TrackGain(track_exists(*track)?)),
            ControlTarget::TrackPan { track } => Ok(ParamTarget::TrackPan(track_exists(*track)?)),
            ControlTarget::MasterGain => Ok(ParamTarget::MasterGain),
            ControlTarget::MasterPan => Ok(ParamTarget::MasterPan),
            ControlTarget::Output {
                source,
                target,
                pre_fader,
            } => {
                let track = match self.graph_node(source)? {
                    crate::mixer_graph::MixerNode::Strip(Some(track)) => track,
                    _ => return Err("source must be a track node from mixer_graph".into()),
                };
                let to = match self.graph_node(target)? {
                    crate::mixer_graph::MixerNode::Strip(owner) => {
                        crate::mixer_graph::destination(owner)
                    }
                    _ => return Err("target must be a bus node or track:master".into()),
                };
                let tap = if *pre_fader {
                    auris_core::Tap::PreFader
                } else {
                    auris_core::Tap::PostFader
                };
                self.project()
                    .track(track)
                    .is_some_and(|entry| entry.outputs.iter().any(|o| o.key() == (to, tap)))
                    .then_some(ParamTarget::Output { track, to, tap })
                    .ok_or("Unknown output; mixer_graph read again".into())
            }
            ControlTarget::Instrument { track, param } => {
                let track = track_exists(*track)?;
                let descriptor = self
                    .instrument_descriptors(track)
                    .iter()
                    .find(|d| d.key.as_ref() == param)
                    .cloned()
                    .ok_or("Unknown instrument parameter key; read inspect_instrument")?;
                Ok(ParamTarget::Instrument {
                    track,
                    param: descriptor.id,
                })
            }
            ControlTarget::Effect { node, param } => {
                let effect = self
                    .mixer_graph()
                    .nodes
                    .into_iter()
                    .find(|n| n.key() == *node)
                    .ok_or("Unknown effect node")?;
                let crate::mixer_graph::MixerNode::Effect(track, slot) = effect else {
                    return Err("Expected effect node".into());
                };
                let descriptor = self
                    .effect_descriptors(slot)
                    .iter()
                    .find(|d| d.key.as_ref() == param)
                    .cloned()
                    .ok_or("Unknown effect parameter key; read mixer_graph parameters")?;
                Ok(ParamTarget::Effect {
                    track,
                    slot,
                    param: descriptor.id,
                })
            }
        }
    }

    /// Read or edit one native-unit automation lane with revision-checked writes.
    pub fn agent_automation(
        &mut self,
        target: ControlTarget,
        operation: Operation,
        expected_revision: Option<String>,
    ) -> Result<String, String> {
        let target = self.agent_control_target(&target)?;
        let descriptor = self
            .descriptor_for(target)
            .ok_or("Parameter is unavailable")?;
        if let Operation::Read {
            offset,
            choice_offset,
        } = operation
        {
            let lane = self.automation().lane(target);
            let points = lane.map(|lane| lane.points()).unwrap_or_default();
            if offset > points.len() || choice_offset > descriptor.choices.len() {
                return Err("Offset exceeds points or choices".into());
            }
            let page: Vec<_> = points
                .iter()
                .skip(offset)
                .take(128)
                .map(|point| serde_json::json!({"at_tick":point.tick.raw(),"value":point.value}))
                .collect();
            let choices: Vec<_> = descriptor
                .choices
                .iter()
                .skip(choice_offset)
                .take(128)
                .collect();
            let next = offset + page.len();
            let choice_next = choice_offset + choices.len();
            let mut report = self.agent_parameter_report(target, &descriptor);
            report["revision"] = self.agent_document_revision()?.into();
            report["curve"] = serde_json::json!(lane.map(|lane| lane.curve));
            report["points"] = serde_json::json!(page);
            report["total"] = points.len().into();
            report["next_offset"] = serde_json::json!((next < points.len()).then_some(next));
            report["choices"] = serde_json::json!(choices);
            report["next_choice_offset"] =
                serde_json::json!((choice_next < descriptor.choices.len()).then_some(choice_next));
            return Ok(report.to_string());
        }
        if descriptor.read_only && matches!(operation, Operation::Set { .. }) {
            return Err("parameter is read-only".into());
        }
        if expected_revision.as_deref() != Some(self.agent_document_revision()?.as_str()) {
            return Err("Read automation first and supply its current revision".into());
        }
        match operation {
            Operation::Clear => {
                self.clear_automation(target);
            }
            Operation::Set {
                points,
                replace,
                curve,
            } => {
                if points.is_empty() || points.len() > 4096 {
                    return Err("Supply 1..4096 automation points".into());
                }
                let points = points
                    .into_iter()
                    .map(|point| {
                        if !point.value.is_finite()
                            || (descriptor.clamp(point.value) - point.value).abs() > 1e-5
                        {
                            return Err(
                                "Values must match native bounds and discrete choices".to_string()
                            );
                        }
                        Ok(AutomationPoint {
                            tick: Ticks(point.at_tick),
                            value: point.value,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let curve = curve.map(|curve| match curve {
                    Interpolation::Linear => AutomationCurve::Linear,
                    Interpolation::Hold => AutomationCurve::Hold,
                });
                self.write_automation_points(target, &points, replace, curve)
                    .map_err(|e| e.to_string())?;
            }
            Operation::Read { .. } => unreachable!(),
        }
        Ok(serde_json::json!({"revision":self.agent_document_revision()?,"automated":self.is_automated(target)}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_gain_points_round_trip_atomically_and_preserve_static_values() {
        let mut session = Session::new(crate::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let target = || ControlTarget::TrackGain { track: track.0 };
        let before = session.project().clone();
        let revision = session.agent_document_revision().unwrap();
        session
            .agent_automation(
                target(),
                Operation::Set {
                    points: vec![
                        Point {
                            at_tick: 13,
                            value: -12.0,
                        },
                        Point {
                            at_tick: 997,
                            value: -6.0,
                        },
                    ],
                    replace: true,
                    curve: Some(Interpolation::Linear),
                },
                Some(revision.clone()),
            )
            .unwrap();
        let report: serde_json::Value = serde_json::from_str(
            &session
                .agent_automation(
                    target(),
                    Operation::Read {
                        offset: 0,
                        choice_offset: 0,
                    },
                    None,
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(report["points"][0]["at_tick"], 13);
        assert_eq!(report["points"][1]["value"], -6.0);
        assert_eq!(
            session.project().track(track).unwrap().mixer.gain_db,
            before.track(track).unwrap().mixer.gain_db
        );
        let after = session.project().clone();
        assert!(
            session
                .agent_automation(target(), Operation::Clear, Some(revision))
                .is_err()
        );
        assert_eq!(session.project(), &after);
        session.undo();
        assert_eq!(session.project(), &before);
        let revision = session.agent_document_revision().unwrap();
        assert!(
            session
                .agent_automation(
                    target(),
                    Operation::Set {
                        points: vec![
                            Point {
                                at_tick: 0,
                                value: -12.0
                            },
                            Point {
                                at_tick: 997,
                                value: 900.0
                            }
                        ],
                        replace: true,
                        curve: None
                    },
                    Some(revision)
                )
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }
}
