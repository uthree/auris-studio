//! Parameter types and utilities for VST3 host

use crate::Result;
use serde::{Deserialize, Serialize};

/// Plugin parameter information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    /// Parameter ID
    pub id: u32,
    /// Parameter name
    pub name: String,
    /// Current normalized value (0.0 to 1.0)
    pub value: f64,
    /// Minimum value in normalized space (VST3 parameters are always 0.0..=1.0;
    /// the plain/engineering range is private to the plugin — use
    /// [`crate::Plugin::format_parameter`] for human-readable values).
    pub min: f64,
    /// Maximum value in normalized space (always 1.0 for VST3 parameters).
    pub max: f64,
    /// Default value
    pub default: f64,
    /// Parameter unit (e.g., "Hz", "dB", "%")
    pub unit: String,
    /// Step count, straight from VST3 `ParameterInfo::stepCount`, which counts the *gaps* between
    /// discrete values rather than the values themselves: `0` is continuous, `1` is a two-state
    /// toggle, and `n` is a list of `n + 1` values. So a three-way selector reports `2`, not `3`.
    pub step_count: i32,
    /// Whether the parameter can be automated
    pub can_automate: bool,
    /// Whether the parameter is read-only
    pub is_read_only: bool,
    /// Whether the parameter is a bypass control
    pub is_bypass: bool,
    /// Parameter flags
    pub flags: u32,
}

impl Parameter {
    /// Convert normalized value (0.0-1.0) to plain value
    pub fn normalized_to_plain(&self, normalized: f64) -> f64 {
        if self.step_count >= 1 {
            let step = self.step_index(normalized).unwrap_or_default() as f64;
            self.min + (step / self.step_count as f64) * (self.max - self.min)
        } else {
            // Continuous parameter
            self.min + normalized * (self.max - self.min)
        }
    }

    /// Which discrete value a normalized position selects, for a stepped parameter: `0..=step_count`
    /// (so `step_count + 1` possible values). `None` for a continuous parameter.
    pub fn step_index(&self, normalized: f64) -> Option<i32> {
        if self.step_count >= 1 {
            let value = normalized.clamp(0.0, 1.0);
            // `step_count` comes straight from the plugin, so the "+1 values" arithmetic is done
            // in `f64` — `step_count + 1` on an `i32` overflows for a plugin that reports
            // `i32::MAX`. The float-to-int cast saturates, and `min` puts it back in range.
            Some(((value * (f64::from(self.step_count) + 1.0)) as i32).min(self.step_count))
        } else {
            None
        }
    }

    /// Convert plain value to normalized value (0.0-1.0)
    pub fn plain_to_normalized(&self, plain: f64) -> f64 {
        if (self.max - self.min).abs() < f64::EPSILON {
            0.0
        } else {
            ((plain - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }

    /// Approximate a human-readable value string from normalized space.
    ///
    /// This cannot know the plugin's internal mapping (VST3 keeps that private), so
    /// for continuous parameters it just reports the normalized number with the unit.
    /// For accurate display (e.g. `"440.00 Hz"`), use
    /// [`crate::Plugin::format_parameter`], which asks the plugin to format it.
    pub fn format_value(&self, normalized: f64) -> String {
        match self.step_index(normalized) {
            // Toggle: one step, so two states.
            Some(index) if self.step_count == 1 => {
                if index >= 1 { "On" } else { "Off" }.to_string()
            }
            // Stepped: report which value is selected. The plain value lives in normalized space
            // (VST3 keeps the engineering range private), so the index is the only meaningful
            // number to show — use `Plugin::format_parameter` for the plugin's own label.
            Some(index) => {
                if self.unit.is_empty() {
                    format!("{index}")
                } else {
                    format!("{} {}", index, self.unit)
                }
            }
            None => {
                let plain = self.normalized_to_plain(normalized);
                if self.unit.is_empty() {
                    format!("{plain:.3}")
                } else {
                    format!("{:.3} {}", plain, self.unit)
                }
            }
        }
    }

    /// Whether this parameter takes discrete steps rather than a continuous range.
    ///
    /// True for toggles too — a toggle is just the one-step case. See [`Self::step_count`] for the
    /// VST3 counting convention.
    pub fn is_discrete(&self) -> bool {
        self.step_count >= 1
    }

    /// Whether this parameter is a two-state toggle (VST3 `stepCount == 1`).
    pub fn is_boolean(&self) -> bool {
        self.step_count == 1
    }
}

/// Parameter change event
#[derive(Debug, Clone)]
pub struct ParameterChange {
    /// Parameter ID
    pub id: u32,
    /// New normalized value (0.0 to 1.0)
    pub value: f64,
    /// Sample offset within the current block
    pub sample_offset: i32,
}

/// Batch parameter update
pub struct ParameterUpdate<'a> {
    updates: Vec<(u32, f64)>,
    plugin: &'a mut crate::Plugin,
}

impl<'a> ParameterUpdate<'a> {
    pub(crate) fn new(plugin: &'a mut crate::Plugin) -> Self {
        Self {
            updates: Vec::new(),
            plugin,
        }
    }

    /// Set a parameter value
    pub fn set(&mut self, id: u32, value: f64) -> &mut Self {
        self.updates.push((id, value));
        self
    }

    /// Apply the queued parameter updates, in the order they were [`set`](Self::set).
    ///
    /// # This batch is not atomic
    ///
    /// The first failure stops the batch and is returned. The updates queued *before* it have
    /// already reached the plugin and are **not** rolled back; the ones after it were never
    /// attempted, and the error does not say how far the batch got. Call
    /// [`Plugin::set_parameter`](crate::Plugin::set_parameter) per parameter if you need to
    /// know which landed, or re-read them with
    /// [`Plugin::get_parameters`](crate::Plugin::get_parameters) afterwards.
    pub fn apply(self) -> Result<()> {
        for (id, value) in self.updates {
            self.plugin.set_parameter(id, value)?;
        }
        Ok(())
    }
}

/// Parameter automation curve types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutomationCurve {
    /// Linear interpolation
    Linear,
    /// Exponential curve
    Exponential,
    /// Logarithmic curve
    Logarithmic,
    /// Step (no interpolation)
    Step,
}

/// Parameter automation point
#[derive(Debug, Clone)]
pub struct AutomationPoint {
    /// Time in seconds
    pub time: f64,
    /// Normalized value (0.0 to 1.0)
    pub value: f64,
    /// Curve type to next point
    pub curve: AutomationCurve,
}

/// Parameter automation data
#[derive(Debug, Clone)]
pub struct ParameterAutomation {
    /// Automation points
    pub points: Vec<AutomationPoint>,
    /// Whether to loop the automation
    pub looping: bool,
}

impl ParameterAutomation {
    /// Create new automation
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            looping: false,
        }
    }

    /// Add an automation point
    pub fn add_point(mut self, time: f64, value: f64) -> Self {
        self.points.push(AutomationPoint {
            time,
            value,
            curve: AutomationCurve::Linear,
        });
        // `total_cmp` orders NaN deterministically instead of panicking like
        // `partial_cmp(..).unwrap()` would on a NaN time from this public API.
        self.points.sort_by(|a, b| a.time.total_cmp(&b.time));
        self
    }

    /// Set the curve type
    pub fn with_curve(mut self, curve: AutomationCurve) -> Self {
        for point in &mut self.points {
            point.curve = curve;
        }
        self
    }

    /// Enable looping
    pub fn with_loop(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    /// Get value at specific time
    pub fn value_at_time(&self, time: f64) -> Option<f64> {
        if self.points.is_empty() {
            return None;
        }

        // Handle looping
        let time = if self.looping && !self.points.is_empty() {
            let duration = self.points.last().unwrap().time;
            if duration > 0.0 {
                time % duration
            } else {
                time
            }
        } else {
            time
        };

        // Find surrounding points
        let mut prev = None;
        let mut next = None;

        for (i, point) in self.points.iter().enumerate() {
            if point.time <= time {
                prev = Some(i);
            } else {
                next = Some(i);
                break;
            }
        }

        // Every branch clamps to the normalized range and maps non-finite to a usable value.
        // `add_point` accepts any `f64`, and the consumers don't tolerate out-of-range input:
        // `Plugin::set_parameter_at` rejects it, and `Timeline::drive_block` propagates that
        // rejection *before* processing audio — so one bad automation point stopped rendering
        // entirely, and the block's MIDI (already consumed by `advance_block`) was lost with it.
        fn sanitize(value: f64) -> f64 {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            }
        }

        match (prev, next) {
            (None, _) => Some(sanitize(self.points[0].value)),
            (Some(i), None) => Some(sanitize(self.points[i].value)),
            (Some(i), Some(j)) => {
                let p1 = &self.points[i];
                let p2 = &self.points[j];

                let t = (time - p1.time) / (p2.time - p1.time);

                let value = match p1.curve {
                    AutomationCurve::Linear => p1.value + (p2.value - p1.value) * t,
                    AutomationCurve::Exponential => p1.value + (p2.value - p1.value) * t * t,
                    AutomationCurve::Logarithmic => p1.value + (p2.value - p1.value) * t.sqrt(),
                    AutomationCurve::Step => p1.value,
                };

                Some(sanitize(value))
            }
        }
    }

    /// Sample this automation across one audio block, returning `(sample_offset, value)`
    /// points suitable for sample-accurate scheduling (e.g. [`Plugin::set_parameter_at`]).
    ///
    /// `block_start_secs` is the block's start on the automation timeline; `frames` is the
    /// block length; `points_per_block` is the sub-block resolution (1 = one value at the
    /// block start; higher = finer ramps, capped at `frames`). Returns empty if the
    /// automation has no points.
    ///
    /// [`Plugin::set_parameter_at`]: crate::Plugin::set_parameter_at
    pub fn points_for_block(
        &self,
        block_start_secs: f64,
        frames: usize,
        sample_rate: f64,
        points_per_block: usize,
    ) -> Vec<(i32, f64)> {
        if self.points.is_empty() || frames == 0 {
            return Vec::new();
        }
        let n = points_per_block.clamp(1, frames);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            // Widened: `i * frames` overflows `usize` for an absurd `frames`, and this is
            // reachable from `Timeline::advance_block`'s caller-supplied block length.
            let offset = ((i as u128 * frames as u128) / n as u128) as usize;
            let time = block_start_secs + offset as f64 / sample_rate;
            if let Some(value) = self.value_at_time(time) {
                // Saturating: a `frames` past `i32::MAX` wraps into a negative sample offset,
                // which `Plugin::set_parameter_at` would carry into the plugin's event list.
                out.push((offset.min(i32::MAX as usize) as i32, value));
            }
        }
        out
    }
}

impl Default for ParameterAutomation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every branch of `value_at_time` must return a usable normalized value. Only the
    /// interpolating branch clamped, so a curve whose first or last point was out of range handed
    /// that raw value straight to `Plugin::set_parameter_at`, which rejects it — and
    /// `Timeline::drive_block` propagates the rejection *before* processing audio, so rendering
    /// stopped dead once the playhead reached the last point (losing that block's MIDI too).
    #[test]
    fn value_at_time_is_always_a_valid_normalized_value() {
        let a = ParameterAutomation::new()
            .add_point(0.0, 99.0)
            .add_point(1.0, -50.0);
        // Before the first point, exactly on it, between, on the last, and past it.
        for t in [-1.0, 0.0, 0.5, 1.0, 2.0] {
            let v = a.value_at_time(t).expect("some value");
            assert!(
                (0.0..=1.0).contains(&v),
                "value_at_time({t}) = {v}, outside 0..=1"
            );
        }

        // A NaN point time must not produce NaN values either.
        let b = ParameterAutomation::new()
            .add_point(0.0, 0.0)
            .add_point(f64::NAN, 1.0);
        for t in [0.0, 0.5, 1.0] {
            let v = b.value_at_time(t).expect("some value");
            assert!(v.is_finite(), "value_at_time({t}) = {v}");
            assert!((0.0..=1.0).contains(&v), "value_at_time({t}) = {v}");
        }

        // And the block sampler inherits it.
        for (offset, value) in b.points_for_block(0.0, 2, 1.0, 2) {
            assert!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "{offset} -> {value}"
            );
        }
    }

    /// `frames` reaches here from `Timeline::advance_block`, so the sub-block offset maths must
    /// not overflow on an absurd block length — and the offsets it emits must stay a valid
    /// sample offset, not a value wrapped negative by the `as i32` cast.
    #[test]
    fn points_for_block_survives_an_absurd_block_length() {
        let a = ParameterAutomation::new()
            .add_point(0.0, 0.0)
            .add_point(1.0, 1.0);
        let points = a.points_for_block(0.0, usize::MAX, 48_000.0, 4);
        assert!(points.iter().all(|(_, v)| v.is_finite()));
        assert!(
            points.iter().all(|(offset, _)| *offset >= 0),
            "offsets wrapped negative: {points:?}"
        );
        // The offsets are still ordered, saturating at the largest offset VST3 can express.
        assert!(points.windows(2).all(|w| w[0].0 <= w[1].0));
        assert_eq!(points.last().map(|(offset, _)| *offset), Some(i32::MAX));
    }

    #[test]
    fn add_point_with_nan_time_does_not_panic() {
        // A NaN time is ordered deterministically by `total_cmp` in the sort, never panicking.
        let auto = ParameterAutomation::new()
            .add_point(0.0, 0.1)
            .add_point(f64::NAN, 0.5)
            .add_point(1.0, 0.9);
        assert_eq!(auto.points.len(), 3);

        // Sane ordering: the finite points keep ascending-time order, and the NaN is placed
        // deterministically (`total_cmp` sorts a positive NaN after all finite values) rather
        // than corrupting the sequence or panicking.
        let finite: Vec<f64> = auto
            .points
            .iter()
            .map(|p| p.time)
            .filter(|t| t.is_finite())
            .collect();
        assert_eq!(finite, vec![0.0, 1.0]);
        assert!(auto.points.last().unwrap().time.is_nan());
    }

    fn stepped(step_count: i32) -> Parameter {
        Parameter {
            id: 0,
            name: "stepped".to_string(),
            value: 0.0,
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: String::new(),
            step_count,
            can_automate: true,
            is_read_only: false,
            is_bypass: false,
            flags: 0,
        }
    }

    /// `step_count` is whatever the plugin reported, so the `+ 1` for "step_count + 1 values"
    /// has to survive `i32::MAX` — in `i32` that arithmetic overflows and panics in debug.
    #[test]
    fn step_index_survives_an_absurd_step_count() {
        let param = stepped(i32::MAX);
        for normalized in [0.0, 0.5, 1.0] {
            let index = param.step_index(normalized).expect("stepped");
            assert!(
                (0..=i32::MAX).contains(&index),
                "step_index({normalized}) = {index}"
            );
        }
        assert_eq!(param.step_index(1.0), Some(i32::MAX));
        // And `format_value`, which goes through the same maths, still renders.
        assert!(!param.format_value(1.0).is_empty());
    }

    #[test]
    fn step_index_covers_every_value_of_an_ordinary_stepped_parameter() {
        let param = stepped(2); // three values
        assert_eq!(param.step_index(0.0), Some(0));
        assert_eq!(param.step_index(0.5), Some(1));
        assert_eq!(param.step_index(1.0), Some(2));
        assert_eq!(stepped(0).step_index(0.5), None);
    }

    #[test]
    fn add_point_with_nan_value_is_not_used_in_ordering() {
        // The sort keys on time only, so a NaN *value* can never reach the comparator and
        // can never break ordering or panic. Lock that in.
        let auto = ParameterAutomation::new()
            .add_point(2.0, f64::NAN)
            .add_point(1.0, 0.5)
            .add_point(0.0, 0.25);
        let times: Vec<f64> = auto.points.iter().map(|p| p.time).collect();
        assert_eq!(times, vec![0.0, 1.0, 2.0]);
    }
}
