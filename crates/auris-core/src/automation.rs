//! Parameter values that move along the timeline.
//!
//! The fifth of the document's timeline maps, beside the tempo, the meter, the key and the chords
//! — and the first one that is a *set* of maps rather than a single one, because there is a curve
//! per parameter rather than one per project. What they share is the shape: sorted points, a
//! value answered at any tick, and deserialisation routed through a repr so a hand-edited file
//! cannot produce a lane the readers would trip over.
//!
//! Two things differ from the tempo map, and both come out of what a parameter is.
//!
//! There is **no anchor at tick 0**. A lane can begin at bar 40 because that is where the fade
//! starts, and before its first point it simply holds that point's value — a tempo has to be
//! defined from the first sample, a filter cutoff does not.
//!
//! A lane carries **how to get between its points**. A fader wants a straight line; a waveform
//! chooser wants to hold, because sweeping from square to saw would pass through every waveform
//! in between and play all of them. Which one a parameter gets is decided where the descriptor is
//! legible — [`ParamDescriptor::steps`](crate::param::ParamDescriptor::steps) is the tell — and
//! then stored, so the document does not need a plugin loaded to be read correctly.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::param::{ParamId, ParamTarget};
use crate::project::TrackId;
use crate::time::Ticks;

/// How a lane gets from one point to the next.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutomationCurve {
    /// A straight line between the two values. What a fader, a cutoff or a mix control wants.
    #[default]
    Linear,
    /// Hold the earlier value until the next point, then jump. What a switch or a choice list
    /// wants: there is nothing between "square" and "saw" to be half way through.
    Hold,
}

/// One value written at one point on the timeline.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationPoint {
    /// Where on the timeline the value is written.
    pub tick: Ticks,
    /// The value itself, in the parameter's own units — decibels for a fader, hertz for a
    /// cutoff. Not normalised, so a lane read without its descriptor still means something.
    pub value: f32,
}

impl AutomationPoint {
    /// A point, from a tick and a value.
    pub fn new(tick: Ticks, value: f32) -> Self {
        Self { tick, value }
    }
}

/// One parameter's value over the whole timeline.
///
/// Invariants, upheld by every constructor and mutator: at least one point, sorted by tick, no
/// two points at the same tick, every tick at or after zero, and every value finite. A lane that
/// loses its last point is not an empty lane — it is no lane, and [`Automation`] removes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "AutomationLaneRepr")]
pub struct AutomationLane {
    /// Which parameter this lane drives.
    pub target: ParamTarget,
    /// The stable key of the parameter [`Self::target`] addresses, for a plugin's parameter.
    ///
    /// `None` for the mixer's own controls: a fader is addressed by track id, and a track id means
    /// the same thing tomorrow. A plugin parameter is addressed by [`ParamId`], which is a
    /// *position* in the plugin's list — stable for as long as the plugin is, and no longer. A
    /// hosted CLAP plugin reorders its list whenever its author adds a parameter, and this crate's
    /// own module doc has always said what to do about it: address a parameter by number at
    /// runtime and by key on disk. [`PluginState`](crate::plugin::PluginState) already did for the
    /// static values; a lane did not, so a saved curve drawn on Cutoff came back driving whatever
    /// had moved into position twelve.
    ///
    /// Kept beside the target rather than inside it because a [`ParamTarget`] is [`Copy`] and is
    /// spent everywhere as a lightweight name — a session's undo steps are keyed by one. So the key
    /// travels with the lane, and [`Automation::realign_by_key`] is what puts the two back into
    /// agreement when a document is opened.
    key: Option<String>,
    /// How to get from one point to the next.
    pub curve: AutomationCurve,
    points: Vec<AutomationPoint>,
}

/// On-disk shape of an [`AutomationLane`]. It mirrors what `Serialize` emits, so only the
/// validation on the way in is new.
#[derive(Deserialize)]
struct AutomationLaneRepr {
    target: ParamTarget,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    curve: AutomationCurve,
    points: Vec<AutomationPoint>,
}

impl TryFrom<AutomationLaneRepr> for AutomationLane {
    type Error = CoreError;

    fn try_from(repr: AutomationLaneRepr) -> Result<Self> {
        Self::new(repr.target, repr.key, repr.curve, repr.points)
    }
}

impl AutomationLane {
    /// A lane over `target`, from points in any order.
    ///
    /// They are sorted here rather than demanded sorted: the callers that build one are a file
    /// being read and a user dragging a point past its neighbour, and neither can promise it.
    ///
    /// Two points at one tick keep the one written first. Which of them survives is arbitrary —
    /// one instant cannot hold two values — so what matters is that it is decided rather than
    /// left to whichever the reader reached first. A point dragged onto another does not come
    /// through here: [`Self::move_point`] lifts it out and puts it down, so the one that lands
    /// replaces the one that was there.
    pub fn new(
        target: ParamTarget,
        key: Option<String>,
        curve: AutomationCurve,
        mut points: Vec<AutomationPoint>,
    ) -> Result<Self> {
        if points.is_empty() {
            return Err(CoreError::InvalidAutomationLane(
                "an automation lane needs at least one point".into(),
            ));
        }
        if let Some(bad) = points
            .iter()
            .find(|point| !point.value.is_finite() || point.tick < Ticks::ZERO)
        {
            return Err(CoreError::InvalidAutomationLane(format!(
                "automation point at {:?} is not a finite value at a real position",
                bad.tick
            )));
        }
        points.sort_by_key(|point| point.tick);
        points.dedup_by_key(|point| point.tick);
        Ok(Self {
            target,
            key,
            curve,
            points,
        })
    }

    /// A lane holding one point.
    pub fn single(
        target: ParamTarget,
        key: Option<String>,
        curve: AutomationCurve,
        point: AutomationPoint,
    ) -> Self {
        Self {
            target,
            key,
            curve,
            points: vec![point],
        }
    }

    /// The stable key of the parameter this lane drives, for a plugin's parameter.
    ///
    /// See the field's own account of why a lane carries one; `None` for the mixer's own controls,
    /// and for a lane written by a build that had no answer to give.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Every point, in timeline order.
    pub fn points(&self) -> &[AutomationPoint] {
        &self.points
    }

    /// Where the lane's first point sits.
    pub fn start(&self) -> Ticks {
        self.points[0].tick
    }

    /// Where the lane's last point sits.
    pub fn end(&self) -> Ticks {
        self.points[self.points.len() - 1].tick
    }

    /// The value in force at `tick`.
    ///
    /// Held flat before the first point and after the last: a lane says what happens where it was
    /// written and makes no claim about the rest of the song, so the nearest thing it did say is
    /// the honest answer at either end.
    pub fn value_at(&self, tick: Ticks) -> f32 {
        let index = match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(exact) => return self.points[exact].value,
            Err(index) => index,
        };
        if index == 0 {
            return self.points[0].value;
        }
        let before = self.points[index - 1];
        let Some(after) = self.points.get(index) else {
            return before.value;
        };
        match self.curve {
            AutomationCurve::Hold => before.value,
            AutomationCurve::Linear => {
                let span = (after.tick - before.tick).raw();
                // Guarded even though the invariant forbids it: a zero span here would be a
                // division that produces NaN and writes it into a plugin, which is far worse
                // than the wrong value.
                if span <= 0 {
                    return before.value;
                }
                let travelled = (tick - before.tick).raw() as f32 / span as f32;
                before.value + (after.value - before.value) * travelled
            }
        }
    }

    /// Writes a point, replacing whatever was at that tick.
    ///
    /// A non-finite value or a negative tick is refused rather than stored: the first would reach
    /// a plugin and the second would sit before the timeline begins.
    pub fn set_point(&mut self, tick: Ticks, value: f32) -> bool {
        if !value.is_finite() || tick < Ticks::ZERO {
            return false;
        }
        match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(exact) => self.points[exact].value = value,
            Err(index) => self.points.insert(index, AutomationPoint::new(tick, value)),
        }
        true
    }

    /// Removes the point at `tick`, if there is one.
    ///
    /// Removing the last one is allowed and leaves an empty lane, which [`Automation`] then drops
    /// — the emptiness is a moment during a removal rather than a state a document can hold.
    pub fn remove_point(&mut self, tick: Ticks) -> bool {
        match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(exact) => {
                self.points.remove(exact);
                true
            }
            Err(_) => false,
        }
    }

    /// Moves the point at `from` to `to`, taking `value` with it.
    ///
    /// Returns where it ended up, which is not always where it was asked to go: landing on
    /// another point replaces that one, so a drag across a neighbour absorbs it rather than
    /// producing two values at one instant.
    pub fn move_point(&mut self, from: Ticks, to: Ticks, value: f32) -> Option<Ticks> {
        if !value.is_finite() {
            return None;
        }
        let to = to.max_zero();
        if !self.remove_point(from) {
            return None;
        }
        self.set_point(to, value);
        Some(to)
    }

    /// Whether the lane has run out of points and should be dropped.
    fn is_spent(&self) -> bool {
        self.points.is_empty()
    }
}

/// Every automated parameter in the project.
///
/// A list rather than a map keyed by target, because a JSON object's keys are strings and a
/// [`ParamTarget`] is not one. Lookup is a scan, which costs nothing at the sizes involved: a
/// project has tens of lanes, not thousands, and the render graph resolves them once per rebuild
/// rather than once per block.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Automation {
    lanes: Vec<AutomationLane>,
}

impl Automation {
    /// Nothing automated.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any parameter is automated at all.
    pub fn is_empty(&self) -> bool {
        self.lanes.is_empty()
    }

    /// How many parameters are automated.
    pub fn len(&self) -> usize {
        self.lanes.len()
    }

    /// Every lane, in the order they were first written.
    pub fn lanes(&self) -> &[AutomationLane] {
        &self.lanes
    }

    /// The lane driving `target`, if it is automated.
    pub fn lane(&self, target: ParamTarget) -> Option<&AutomationLane> {
        self.lanes.iter().find(|lane| lane.target == target)
    }

    /// The value `target` is driven to at `tick`, or `None` when it is not automated.
    ///
    /// `None` rather than a default is the whole contract: a parameter with no lane keeps the
    /// static value stored on the strip or in the plugin state, and only an existing lane takes
    /// over. That is what lets a project be automated one parameter at a time.
    pub fn value_at(&self, target: ParamTarget, tick: Ticks) -> Option<f32> {
        self.lane(target).map(|lane| lane.value_at(tick))
    }

    /// Writes a point on `target`'s lane, starting the lane if it had none.
    ///
    /// `curve` is only consulted when the lane is being created. Changing how an existing lane
    /// interpolates is [`Self::set_curve`], so writing a point cannot silently restyle a curve
    /// somebody already shaped.
    /// `key` is the parameter's stable key, and is only consulted when the lane is being created —
    /// see [`AutomationLane::key`] for what a lane needs one for. `None` for a mixer control.
    pub fn set_point(
        &mut self,
        target: ParamTarget,
        key: Option<String>,
        curve: AutomationCurve,
        tick: Ticks,
        value: f32,
    ) -> bool {
        if let Some(lane) = self.lanes.iter_mut().find(|lane| lane.target == target) {
            return lane.set_point(tick, value);
        }
        if !value.is_finite() || tick < Ticks::ZERO {
            return false;
        }
        self.lanes.push(AutomationLane::single(
            target,
            key,
            curve,
            AutomationPoint::new(tick, value),
        ));
        true
    }

    /// Re-points every plugin lane at wherever the key it was saved under sits *now*.
    ///
    /// `current` is asked, for one lane's target and key, which [`ParamId`] that key holds today,
    /// and answers `None` for a key the plugin no longer has. A lane that has moved is re-pointed;
    /// a lane whose parameter is gone is dropped, because what it would otherwise do is drive
    /// whatever now occupies its old position — the same reason
    /// `auris_engine`'s graph drops a lane it cannot resolve, and worse than silence for the same
    /// reason. Mixer lanes carry no key and are left alone.
    ///
    /// Returns whether anything moved or was dropped, which is what tells a session the document
    /// has been repaired and is now unsaved.
    pub fn realign_by_key(
        &mut self,
        mut current: impl FnMut(ParamTarget, &str) -> Option<ParamId>,
    ) -> bool {
        let mut changed = false;
        self.lanes.retain_mut(|lane| {
            let Some(key) = lane.key.clone() else {
                return true;
            };
            let Some(was) = lane.target.param() else {
                return true;
            };
            match current(lane.target, &key) {
                Some(now) if now == was => true,
                Some(now) => {
                    lane.target = lane.target.with_param(now);
                    changed = true;
                    true
                }
                None => {
                    changed = true;
                    false
                }
            }
        });
        changed
    }

    /// Changes how an existing lane interpolates.
    pub fn set_curve(&mut self, target: ParamTarget, curve: AutomationCurve) -> bool {
        match self.lanes.iter_mut().find(|lane| lane.target == target) {
            Some(lane) => {
                lane.curve = curve;
                true
            }
            None => false,
        }
    }

    /// Removes one point, and the lane with it when that was the last one.
    pub fn remove_point(&mut self, target: ParamTarget, tick: Ticks) -> bool {
        let Some(index) = self.lanes.iter().position(|lane| lane.target == target) else {
            return false;
        };
        if !self.lanes[index].remove_point(tick) {
            return false;
        }
        if self.lanes[index].is_spent() {
            self.lanes.remove(index);
        }
        true
    }

    /// Moves a point along its lane, returning where it landed.
    pub fn move_point(
        &mut self,
        target: ParamTarget,
        from: Ticks,
        to: Ticks,
        value: f32,
    ) -> Option<Ticks> {
        self.lanes
            .iter_mut()
            .find(|lane| lane.target == target)?
            .move_point(from, to, value)
    }

    /// Removes a whole lane, giving the parameter its static value back.
    pub fn remove_lane(&mut self, target: ParamTarget) -> bool {
        let before = self.lanes.len();
        self.lanes.retain(|lane| lane.target != target);
        self.lanes.len() != before
    }

    /// Drops every lane addressed to `track`.
    ///
    /// Called when a track is deleted. A lane left behind names a track that is not there, and
    /// the id it names will eventually be handed to a different track.
    pub fn remove_track(&mut self, track: TrackId) -> bool {
        let before = self.lanes.len();
        self.lanes.retain(|lane| !lane.target.belongs_to(track));
        self.lanes.len() != before
    }

    /// Drops every lane matching a predicate, for the removals that are not a whole track — an
    /// effect taken out of a chain takes its parameters' curves with it.
    pub fn remove_lanes_where(&mut self, mut doomed: impl FnMut(ParamTarget) -> bool) -> bool {
        let before = self.lanes.len();
        self.lanes.retain(|lane| !doomed(lane.target));
        self.lanes.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::ParamId;
    use crate::project::EffectSlotId;
    use crate::time::TICKS_PER_QUARTER;

    fn beats(n: i64) -> Ticks {
        Ticks(n * TICKS_PER_QUARTER)
    }

    fn fader() -> ParamTarget {
        ParamTarget::TrackGain(TrackId(1))
    }

    fn lane(points: &[(i64, f32)]) -> AutomationLane {
        AutomationLane::new(
            fader(),
            None,
            AutomationCurve::Linear,
            points
                .iter()
                .map(|(beat, value)| AutomationPoint::new(beats(*beat), *value))
                .collect(),
        )
        .expect("a valid lane")
    }

    #[test]
    fn a_lane_holds_its_first_value_backwards_and_its_last_forwards() {
        // A lane makes a claim about the stretch it was written over and none about the rest of
        // the song, so both ends are flat rather than extrapolated.
        let lane = lane(&[(4, -6.0), (8, 0.0)]);
        assert_eq!(lane.value_at(Ticks::ZERO), -6.0);
        assert_eq!(lane.value_at(beats(2)), -6.0);
        assert_eq!(lane.value_at(beats(12)), 0.0);
    }

    #[test]
    fn a_linear_lane_reaches_the_half_way_value_half_way_along() {
        let lane = lane(&[(0, 0.0), (8, 8.0)]);
        assert_eq!(lane.value_at(beats(4)), 4.0);
        assert_eq!(lane.value_at(beats(2)), 2.0);
        assert_eq!(lane.value_at(beats(6)), 6.0);
    }

    #[test]
    fn a_holding_lane_does_not_pass_through_values_nobody_chose() {
        // The reason `Hold` exists: a waveform chooser interpolated from 0 to 3 would play
        // every waveform between them on the way.
        let mut lane = lane(&[(0, 0.0), (8, 3.0)]);
        lane.curve = AutomationCurve::Hold;
        assert_eq!(lane.value_at(beats(4)), 0.0);
        assert_eq!(lane.value_at(beats(7)), 0.0);
        assert_eq!(lane.value_at(beats(8)), 3.0);
    }

    #[test]
    fn a_lane_is_sorted_however_its_points_arrive() {
        let lane = lane(&[(8, 8.0), (0, 0.0), (4, 4.0)]);
        let ticks: Vec<i64> = lane.points().iter().map(|point| point.tick.raw()).collect();
        assert_eq!(ticks, vec![0, beats(4).raw(), beats(8).raw()]);
    }

    #[test]
    fn two_points_at_one_tick_cannot_survive_construction() {
        // One instant cannot hold two values. Which one lives is arbitrary; that it is decided
        // rather than left to the reader is not.
        let lane = lane(&[(4, -3.0), (4, 6.0)]);
        assert_eq!(lane.points().len(), 1);
        assert_eq!(lane.value_at(beats(4)), -3.0, "the one written first");
    }

    #[test]
    fn a_lane_with_no_points_is_refused() {
        assert!(AutomationLane::new(fader(), None, AutomationCurve::Linear, Vec::new()).is_err());
    }

    #[test]
    fn a_value_that_is_not_a_number_never_enters_a_lane() {
        // It would reach a plugin, and `serde_json` writes a non-finite float as `null` — a
        // saved project that can never be opened again.
        assert!(
            AutomationLane::new(
                fader(),
                None,
                AutomationCurve::Linear,
                vec![AutomationPoint::new(Ticks::ZERO, f32::NAN)],
            )
            .is_err()
        );
        let mut lane = lane(&[(0, 0.0)]);
        assert!(!lane.set_point(beats(4), f32::INFINITY));
        assert_eq!(lane.points().len(), 1);
    }

    #[test]
    fn a_point_dragged_onto_another_absorbs_it() {
        let mut lane = lane(&[(0, 0.0), (4, 4.0), (8, 8.0)]);
        let landed = lane.move_point(beats(0), beats(4), 1.0);
        assert_eq!(landed, Some(beats(4)));
        assert_eq!(lane.points().len(), 2);
        assert_eq!(lane.value_at(beats(4)), 1.0);
    }

    #[test]
    fn a_point_dragged_before_the_timeline_lands_at_its_start() {
        let mut lane = lane(&[(4, 4.0), (8, 8.0)]);
        assert_eq!(lane.move_point(beats(4), beats(-2), 4.0), Some(Ticks::ZERO));
        assert_eq!(lane.start(), Ticks::ZERO);
    }

    #[test]
    fn a_parameter_with_no_lane_is_left_to_its_static_value() {
        // The contract that lets a project be automated one parameter at a time: no lane, no
        // answer, and the strip keeps what it was set to.
        let automation = Automation::new();
        assert_eq!(automation.value_at(fader(), beats(4)), None);
    }

    #[test]
    fn writing_a_point_starts_a_lane_and_removing_the_last_one_ends_it() {
        let mut automation = Automation::new();
        assert!(automation.set_point(fader(), None, AutomationCurve::Linear, beats(4), -6.0));
        assert_eq!(automation.len(), 1);
        assert_eq!(automation.value_at(fader(), beats(9)), Some(-6.0));

        assert!(automation.remove_point(fader(), beats(4)));
        assert!(
            automation.is_empty(),
            "a lane with no points is no lane at all"
        );
        assert_eq!(automation.value_at(fader(), beats(4)), None);
    }

    #[test]
    fn writing_a_point_does_not_restyle_a_curve_somebody_shaped() {
        let mut automation = Automation::new();
        automation.set_point(fader(), None, AutomationCurve::Hold, Ticks::ZERO, 0.0);
        // The curve travels with the *creation*; a later write is a value, not a style.
        automation.set_point(fader(), None, AutomationCurve::Linear, beats(8), 8.0);
        assert_eq!(automation.value_at(fader(), beats(4)), Some(0.0));
        assert!(automation.set_curve(fader(), AutomationCurve::Linear));
        assert_eq!(automation.value_at(fader(), beats(4)), Some(4.0));
    }

    #[test]
    fn a_deleted_track_takes_every_lane_pointing_at_it() {
        // Track ids are handed out again, so a lane left behind would eventually drive a
        // parameter on a track that never asked for it.
        let doomed = TrackId(1);
        let survivor = TrackId(2);
        let mut automation = Automation::new();
        for target in [
            ParamTarget::TrackGain(doomed),
            ParamTarget::TrackPan(doomed),
            ParamTarget::Instrument {
                track: doomed,
                param: ParamId(0),
            },
            ParamTarget::Effect {
                track: Some(doomed),
                slot: EffectSlotId(1),
                param: ParamId(0),
            },
            ParamTarget::TrackGain(survivor),
            ParamTarget::MasterGain,
        ] {
            let key = (!target.is_builtin()).then(|| "cutoff".to_string());
            automation.set_point(target, key, AutomationCurve::Linear, Ticks::ZERO, 0.0);
        }
        assert_eq!(automation.len(), 6);
        assert!(automation.remove_track(doomed));
        assert_eq!(automation.len(), 2);
        assert!(automation.lane(ParamTarget::TrackGain(survivor)).is_some());
        assert!(automation.lane(ParamTarget::MasterGain).is_some());
    }

    #[test]
    fn a_lane_follows_its_key_when_the_plugin_moves_the_parameter() {
        // The whole point of a lane carrying a key: a hosted plugin renumbers its list whenever
        // its author inserts a parameter, so the position a lane was saved at names something else
        // by the time the document is opened again.
        let track = TrackId(1);
        let cutoff = ParamTarget::Instrument {
            track,
            param: ParamId(12),
        };
        let mut automation = Automation::new();
        automation.set_point(
            cutoff,
            Some("clap.1042".to_string()),
            AutomationCurve::Linear,
            beats(4),
            800.0,
        );

        // The plugin now reports that key two places further along.
        assert!(automation.realign_by_key(|_, key| match key {
            "clap.1042" => Some(ParamId(14)),
            _ => None,
        }));
        assert!(
            automation.lane(cutoff).is_none(),
            "position twelve is somebody else's parameter now"
        );
        let moved = ParamTarget::Instrument {
            track,
            param: ParamId(14),
        };
        let lane = automation.lane(moved).expect("the lane moved with its key");
        assert_eq!(lane.value_at(beats(4)), 800.0, "and kept its curve");
        assert_eq!(lane.key(), Some("clap.1042"));

        // Asked again with the plugin already agreeing, nothing changes and nothing is reported.
        assert!(!automation.realign_by_key(|_, _| Some(ParamId(14))));
    }

    #[test]
    fn a_lane_whose_parameter_the_plugin_dropped_goes_with_it() {
        // Better than silence is not the test — silence *is* the answer. Left in place the lane
        // would drive whatever moved into its position, which is the failure the engine's own
        // resolver drops a lane to avoid.
        let mut automation = Automation::new();
        automation.set_point(
            ParamTarget::Effect {
                track: None,
                slot: EffectSlotId(3),
                param: ParamId(1),
            },
            Some("clap.7".to_string()),
            AutomationCurve::Linear,
            Ticks::ZERO,
            0.5,
        );
        automation.set_point(fader(), None, AutomationCurve::Linear, Ticks::ZERO, -6.0);

        assert!(automation.realign_by_key(|_, _| None));
        assert_eq!(automation.len(), 1);
        assert!(
            automation.lane(fader()).is_some(),
            "a mixer control carries no key and is nobody's position to move"
        );
    }

    #[test]
    fn a_lane_round_trips_through_json_with_its_invariants_checked() {
        let automation = {
            let mut automation = Automation::new();
            automation.set_point(fader(), None, AutomationCurve::Hold, beats(8), 3.0);
            automation.set_point(fader(), None, AutomationCurve::Hold, beats(0), 0.0);
            automation.set_point(
                ParamTarget::MasterGain,
                None,
                AutomationCurve::Linear,
                beats(4),
                -2.0,
            );
            automation
        };
        let json = serde_json::to_string(&automation).expect("serialises");
        let back: Automation = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back, automation);
        assert_eq!(back.value_at(fader(), beats(4)), Some(0.0));
    }

    #[test]
    fn removing_a_track_from_a_project_takes_its_lanes_with_it() {
        // The rule is enforced where the track actually leaves the document, not by whichever
        // command happened to ask, so no caller can forget the second half.
        use crate::project::Project;
        let mut project = Project::new("Song", 48_000.0);
        let track = project.add_audio_track("Audio 1");
        project.automation.set_point(
            ParamTarget::TrackGain(track),
            None,
            AutomationCurve::Linear,
            Ticks::ZERO,
            -6.0,
        );
        project.automation.set_point(
            ParamTarget::MasterGain,
            None,
            AutomationCurve::Linear,
            Ticks::ZERO,
            0.0,
        );
        assert!(project.remove_track(track));
        assert_eq!(project.automation.len(), 1);
        assert!(project.automation.lane(ParamTarget::MasterGain).is_some());
    }

    #[test]
    fn a_hand_edited_file_cannot_produce_a_lane_the_readers_would_trip_over() {
        // `value_at` indexes `points[0]` without checking, the way every other map here does.
        let empty = r#"[{"target":"MasterGain","curve":"Linear","points":[]}]"#;
        assert!(serde_json::from_str::<Automation>(empty).is_err());
        let unsorted = r#"[{"target":"MasterGain","curve":"Linear","points":[
            {"tick":1920,"value":1.0},{"tick":0,"value":0.0}]}]"#;
        let back: Automation = serde_json::from_str(unsorted).expect("sorted on the way in");
        assert_eq!(
            back.value_at(ParamTarget::MasterGain, Ticks(960)),
            Some(0.5)
        );
    }
}
