//! The document's automation lanes, resolved to where they land in this graph.
//!
//! Its own file because it is a translation table and the one loop that reads it. The document
//! addresses a parameter by id and the graph addresses it by position, so every lookup happens
//! once, here, when the graph is built; what is left for the audio thread is a binary search and a
//! store through the same setters a moved fader uses.

use auris_core::ParamId;
use auris_core::automation::AutomationLane;
use auris_core::param::{ParamTarget, db_to_gain};
use auris_core::project::{EffectSlotId, Project, TrackId};
use auris_core::time::Ticks;

use super::strip::RenderStrip;
use super::track::RenderTrack;

/// Where an automated value goes, in this graph's own coordinates.
///
/// The document addresses a parameter by id; the graph addresses it by position, exactly as every
/// [`EngineCommand`](crate::EngineCommand) does. Translating once when the graph is built rather
/// than once per block is what keeps the audio thread free of lookups — and it turns a lane
/// pointing at something this graph does not have into an absence rather than a miss on every
/// block for the life of the project.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AutomationSlot {
    /// A track's fader, by track index.
    TrackGain(usize),
    /// A track's pan, by track index.
    TrackPan(usize),
    /// The master fader.
    MasterGain,
    /// The master pan.
    MasterPan,
    /// One of a track's send levels, by track index and position in its send list.
    Send { track: usize, send: usize },
    /// A parameter on a track's instrument.
    Instrument { track: usize, param: ParamId },
    /// A parameter on an effect, on a track or on the master bus.
    Effect {
        track: Option<usize>,
        slot: usize,
        param: ParamId,
    },
}

/// One document lane, resolved to where it lands.
pub(super) struct RenderAutomation {
    lane: AutomationLane,
    slot: AutomationSlot,
}

/// Translates every lane in `project` into a graph position, dropping the ones that resolve to
/// nothing.
///
/// A lane can fail to resolve when the thing it names is gone — and it should not be possible,
/// because the document removes a lane along with the track or effect slot it points at. Dropping
/// it here is the second lock on that door: what a lane resolved wrongly would do is drive a
/// parameter on whatever now occupies that position, which is worse than silence.
pub(super) fn resolve_automation(
    project: &Project,
    tracks: &[RenderTrack],
    master: &RenderStrip,
) -> Vec<RenderAutomation> {
    project
        .automation
        .lanes()
        .iter()
        .filter_map(|lane| {
            Some(RenderAutomation {
                lane: lane.clone(),
                slot: resolve_slot(project, tracks, master, lane)?,
            })
        })
        .collect()
}

/// Where one target lands in a graph built from `project`, if it lands anywhere.
fn resolve_slot(
    project: &Project,
    tracks: &[RenderTrack],
    master: &RenderStrip,
    lane: &AutomationLane,
) -> Option<AutomationSlot> {
    let target = lane.target;
    let chain_position = |track: Option<TrackId>, slot: EffectSlotId| {
        let effects = match track {
            Some(id) => &project.track(id)?.mixer.effects,
            None => &project.master.effects,
        };
        effects.iter().position(|effect| effect.id == slot)
    };
    match target {
        ParamTarget::TrackGain(id) => project.track_index(id).map(AutomationSlot::TrackGain),
        ParamTarget::TrackPan(id) => project.track_index(id).map(AutomationSlot::TrackPan),
        ParamTarget::MasterGain => Some(AutomationSlot::MasterGain),
        ParamTarget::MasterPan => Some(AutomationSlot::MasterPan),
        ParamTarget::Send { track, send } => Some(AutomationSlot::Send {
            track: project.track_index(track)?,
            send: project
                .track(track)?
                .sends
                .iter()
                .position(|existing| existing.id == send)?,
        }),
        ParamTarget::Instrument { track, param } => {
            let index = project.track_index(track)?;
            let resolved = lane
                .key()
                .and_then(|key| tracks.get(index)?.instrument_param_for_key(key))
                .or_else(|| lane.key().is_none().then_some(param))?;
            Some(AutomationSlot::Instrument {
                track: index,
                param: resolved,
            })
        }
        ParamTarget::Effect { track, slot, param } => {
            let index = match track {
                Some(id) => Some(project.track_index(id)?),
                None => None,
            };
            let resolved = lane
                .key()
                .and_then(|key| match index {
                    Some(index) => tracks
                        .get(index)?
                        .strip
                        .effect_param_for_key(chain_position(track, slot)?, key),
                    None => master.effect_param_for_key(chain_position(track, slot)?, key),
                })
                .or_else(|| lane.key().is_none().then_some(param))?;
            Some(AutomationSlot::Effect {
                track: index,
                slot: chain_position(track, slot)?,
                param: resolved,
            })
        }
    }
}

/// Writes every lane's value at `tick` into the strip or plugin it drives.
///
/// Free rather than a method so it can be given the tracks and the lanes as separate borrows, and
/// so the rule can be asserted on a handful of structs instead of a whole graph.
///
/// Every value goes in through the same setter a moved fader or a turned knob uses. That is the
/// point: there is no second path into a parameter that could clamp differently, skip the ramp, or
/// forget to tell the pan law.
///
/// `continuing` is false when the playhead arrived rather than advanced — the first segment of a
/// graph, a seek, a loop wrap — and the faders are then put where the lane says instead of sliding
/// there. Only the mixer's own controls have a ramp to skip; a plugin parameter is written the
/// same way either way, because there is nothing between two values of it.
pub(super) fn drive_automation(
    automation: &[RenderAutomation],
    tracks: &mut [RenderTrack],
    master: &mut RenderStrip,
    tick: Ticks,
    continuing: bool,
) {
    for entry in automation {
        let value = entry.lane.value_at(tick);
        match entry.slot {
            AutomationSlot::TrackGain(index) => {
                if let Some(track) = tracks.get_mut(index) {
                    match continuing {
                        true => track.strip.set_gain_db(value),
                        false => track.strip.jump_gain_db(value),
                    }
                }
            }
            AutomationSlot::TrackPan(index) => {
                if let Some(track) = tracks.get_mut(index) {
                    match continuing {
                        true => track.strip.set_pan(value),
                        false => track.strip.jump_pan(value),
                    }
                }
            }
            AutomationSlot::MasterGain => match continuing {
                true => master.set_gain_db(value),
                false => master.jump_gain_db(value),
            },
            AutomationSlot::MasterPan => match continuing {
                true => master.set_pan(value),
                false => master.jump_pan(value),
            },
            AutomationSlot::Send { track, send } => {
                if let Some(send) = tracks
                    .get_mut(track)
                    .and_then(|track| track.sends.get_mut(send))
                {
                    match continuing {
                        true => send.gain.set_target(db_to_gain(value)),
                        false => send.gain.jump_to(db_to_gain(value)),
                    }
                }
            }
            AutomationSlot::Instrument { track, param } => {
                if let Some(track) = tracks.get_mut(track) {
                    track.set_instrument_param(param, value);
                }
            }
            AutomationSlot::Effect { track, slot, param } => match track {
                Some(index) => {
                    if let Some(track) = tracks.get_mut(index) {
                        track.strip.set_effect_param(slot, param, value);
                    }
                }
                None => master.set_effect_param(slot, param, value),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{RenderGraph, RenderSource};
    use crate::testkit;
    use auris_core::automation::AutomationCurve;
    use auris_core::project::{AudioSourceBank, Project};

    #[test]
    fn keyed_instrument_lane_uses_runtime_descriptor_not_saved_numeric_id() {
        let mut project = Project::new("automation", 48_000.0);
        let track = project.add_instrument_track("tone", testkit::TONE_ID);
        let effect = project
            .add_effect(Some(track), testkit::GAIN_ID)
            .expect("effect");
        let master = project
            .add_effect(None, testkit::GAIN_ID)
            .expect("master effect");
        project.automation.set_point(
            ParamTarget::Instrument {
                track,
                param: ParamId(99),
            },
            Some("amplitude".into()),
            AutomationCurve::Hold,
            Ticks::ZERO,
            0.25,
        );
        project.automation.set_point(
            ParamTarget::Effect {
                track: Some(track),
                slot: effect,
                param: ParamId(99),
            },
            Some("gain".into()),
            AutomationCurve::Hold,
            Ticks::ZERO,
            2.0,
        );
        project.automation.set_point(
            ParamTarget::Effect {
                track: None,
                slot: master,
                param: ParamId(99),
            },
            Some("gain".into()),
            AutomationCurve::Hold,
            Ticks::ZERO,
            3.0,
        );
        let mut graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        graph.apply_automation(0, 1);
        let RenderSource::Instrument { instrument, .. } = graph.tracks()[0].source() else {
            panic!("expected instrument");
        };
        assert_eq!(instrument.param(ParamId(0)), 0.25);
        assert_eq!(graph.tracks()[0].strip().effects[0].param(ParamId(0)), 2.0);
        assert_eq!(graph.master().effects[0].param(ParamId(0)), 3.0);
        assert_eq!(graph.automated_count(), 3);
    }

    #[test]
    fn missing_keyed_instrument_and_effect_lanes_are_dropped_instead_of_colliding() {
        let mut project = Project::new("automation", 48_000.0);
        let track = project.add_instrument_track("tone", testkit::TONE_ID);
        let effect = project
            .add_effect(Some(track), testkit::GAIN_ID)
            .expect("effect");
        let master = project
            .add_effect(None, testkit::GAIN_ID)
            .expect("master effect");
        project.automation.set_point(
            ParamTarget::Instrument {
                track,
                param: ParamId(0),
            },
            Some("not-a-real-key".into()),
            AutomationCurve::Hold,
            Ticks::ZERO,
            0.25,
        );
        project.automation.set_point(
            ParamTarget::Effect {
                track: Some(track),
                slot: effect,
                param: ParamId(0),
            },
            Some("not-a-real-key".into()),
            AutomationCurve::Hold,
            Ticks::ZERO,
            2.0,
        );
        project.automation.set_point(
            ParamTarget::Effect {
                track: None,
                slot: master,
                param: ParamId(0),
            },
            Some("not-a-real-key".into()),
            AutomationCurve::Hold,
            Ticks::ZERO,
            3.0,
        );
        let mut graph =
            RenderGraph::build(&project, &AudioSourceBank::new(), &testkit::registry(), 512);
        graph.apply_automation(0, 1);
        let RenderSource::Instrument { instrument, .. } = graph.tracks()[0].source() else {
            panic!("expected instrument");
        };
        assert_eq!(instrument.param(ParamId(0)), testkit::TONE_AMPLITUDE);
        assert_eq!(graph.tracks()[0].strip().effects[0].param(ParamId(0)), 1.0);
        assert_eq!(graph.master().effects[0].param(ParamId(0)), 1.0);
        assert_eq!(graph.automated_count(), 0);
    }
}
