//! Plugin parameter descriptions.
//!
//! Every instrument and effect exposes a flat list of `f32` parameters. Each one is described
//! once by a [`ParamDescriptor`] that carries the range, default, display unit and the mapping
//! curve used by knobs and sliders. The UI can therefore render controls for *any* plugin —
//! including ones added later — without knowing anything about it.
//!
//! Parameters are addressed by a numeric [`ParamId`] at runtime (cheap, stable within a plugin
//! build) and by a string key when saved to disk (stable across builds and reorderings).

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Identifies a parameter within one plugin instance.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct ParamId(pub u32);

impl ParamId {
    /// Raw index.
    pub fn raw(self) -> u32 {
        self.0
    }

    /// Index into a `&[ParamDescriptor]` slice.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl From<u32> for ParamId {
    fn from(value: u32) -> Self {
        ParamId(value)
    }
}

/// How a parameter's value is displayed and edited.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamUnit {
    /// A bare number.
    Plain,
    /// Decibels; already stored in dB.
    Decibels,
    /// Frequency in Hz.
    Hertz,
    /// Time in seconds.
    Seconds,
    /// Time in milliseconds.
    Milliseconds,
    /// A 0..1 value shown as 0..100 %.
    Percent,
    /// A -1..1 stereo position.
    Pan,
    /// Pitch offset in semitones.
    Semitones,
    /// A compression-style `n:1` ratio.
    Ratio,
    /// Beats per minute.
    Bpm,
    /// A discrete choice; `steps` on the descriptor gives the option count.
    Choice,
    /// An on/off switch stored as 0.0 or 1.0.
    Toggle,
}

/// The mapping between a control's 0..1 travel and the parameter's real value.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParamValueCurve {
    /// Value moves proportionally with the control.
    Linear,
    /// Value moves geometrically — the right choice for frequencies and times, where a
    /// fixed *ratio* rather than a fixed *difference* sounds like a constant step.
    /// Requires a strictly positive range.
    Logarithmic,
    /// `value = min + (max - min) * t.powf(exponent)`; use an exponent above 1 to give the
    /// low end of the range more of the control's travel.
    Power(f32),
}

/// Everything the host and UI need to know about one parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDescriptor {
    /// Runtime index; must equal this descriptor's position in the plugin's parameter slice.
    pub id: ParamId,
    /// Stable identifier used when the value is written to a project file.
    pub key: Cow<'static, str>,
    /// Human-readable label.
    pub name: Cow<'static, str>,
    /// Lowest accepted value.
    pub min: f32,
    /// Highest accepted value.
    pub max: f32,
    /// Value a freshly created plugin starts at.
    pub default: f32,
    /// Display unit.
    pub unit: ParamUnit,
    /// Control mapping curve.
    pub curve: ParamValueCurve,
    /// Number of discrete positions, for switches and choice lists.
    pub steps: Option<u32>,
    /// Labels for a [`ParamUnit::Choice`] parameter, in value order.
    pub choices: Cow<'static, [Cow<'static, str>]>,
}

impl ParamDescriptor {
    /// A continuous linear parameter.
    pub fn new(
        id: impl Into<ParamId>,
        key: &'static str,
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
    ) -> Self {
        Self {
            id: id.into(),
            key: Cow::Borrowed(key),
            name: Cow::Borrowed(name),
            min,
            max,
            default,
            unit: ParamUnit::Plain,
            curve: ParamValueCurve::Linear,
            steps: None,
            choices: Cow::Borrowed(&[]),
        }
    }

    /// Sets the display unit.
    pub fn with_unit(mut self, unit: ParamUnit) -> Self {
        self.unit = unit;
        self
    }

    /// Sets the control mapping curve.
    pub fn with_curve(mut self, curve: ParamValueCurve) -> Self {
        self.curve = curve;
        self
    }

    /// Marks the parameter as having `steps` discrete positions.
    pub fn with_steps(mut self, steps: u32) -> Self {
        self.steps = Some(steps.max(1));
        self
    }

    /// Turns the parameter into a labelled choice list.
    pub fn with_choices(mut self, choices: &'static [Cow<'static, str>]) -> Self {
        self.steps = Some(choices.len().max(1) as u32);
        self.unit = ParamUnit::Choice;
        self.max = (choices.len().max(1) - 1) as f32;
        self.min = 0.0;
        self.choices = Cow::Borrowed(choices);
        self
    }

    /// Shorthand for a decibel parameter with a mildly expanded top end.
    pub fn decibels(
        id: impl Into<ParamId>,
        key: &'static str,
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
    ) -> Self {
        Self::new(id, key, name, min, max, default).with_unit(ParamUnit::Decibels)
    }

    /// Shorthand for a logarithmically mapped frequency parameter.
    pub fn hertz(
        id: impl Into<ParamId>,
        key: &'static str,
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
    ) -> Self {
        Self::new(id, key, name, min, max, default)
            .with_unit(ParamUnit::Hertz)
            .with_curve(ParamValueCurve::Logarithmic)
    }

    /// Shorthand for a 0..1 parameter displayed as a percentage.
    pub fn percent(
        id: impl Into<ParamId>,
        key: &'static str,
        name: &'static str,
        default: f32,
    ) -> Self {
        Self::new(id, key, name, 0.0, 1.0, default).with_unit(ParamUnit::Percent)
    }

    /// Shorthand for an on/off parameter.
    pub fn toggle(
        id: impl Into<ParamId>,
        key: &'static str,
        name: &'static str,
        default: bool,
    ) -> Self {
        Self::new(id, key, name, 0.0, 1.0, if default { 1.0 } else { 0.0 })
            .with_unit(ParamUnit::Toggle)
            .with_steps(2)
    }

    /// Clamps `value` into the descriptor's range, snapping to steps when discrete.
    pub fn clamp(&self, value: f32) -> f32 {
        let value = if value.is_finite() {
            value
        } else {
            self.default
        };
        let value = value.clamp(self.min, self.max);
        match self.steps {
            Some(steps) if steps > 1 => {
                let span = self.max - self.min;
                if span.abs() < f32::EPSILON {
                    self.min
                } else {
                    let quantum = span / (steps - 1) as f32;
                    self.min + ((value - self.min) / quantum).round() * quantum
                }
            }
            _ => value,
        }
    }

    /// Maps a real value onto a control's 0..1 travel.
    pub fn normalize(&self, value: f32) -> f32 {
        let value = self.clamp(value);
        match self.curve {
            ParamValueCurve::Logarithmic if self.min > 0.0 && self.max > 0.0 => {
                let span = (self.max / self.min).ln();
                if span.abs() < f32::EPSILON {
                    0.0
                } else {
                    ((value / self.min).ln() / span).clamp(0.0, 1.0)
                }
            }
            ParamValueCurve::Power(exponent) if exponent > 0.0 => {
                let span = self.max - self.min;
                if span.abs() < f32::EPSILON {
                    0.0
                } else {
                    ((value - self.min) / span)
                        .powf(1.0 / exponent)
                        .clamp(0.0, 1.0)
                }
            }
            _ => {
                let span = self.max - self.min;
                if span.abs() < f32::EPSILON {
                    0.0
                } else {
                    ((value - self.min) / span).clamp(0.0, 1.0)
                }
            }
        }
    }

    /// Inverse of [`Self::normalize`].
    pub fn denormalize(&self, normalized: f32) -> f32 {
        let t = normalized.clamp(0.0, 1.0);
        let value = match self.curve {
            ParamValueCurve::Logarithmic if self.min > 0.0 && self.max > 0.0 => {
                self.min * (self.max / self.min).powf(t)
            }
            ParamValueCurve::Power(exponent) if exponent > 0.0 => {
                self.min + (self.max - self.min) * t.powf(exponent)
            }
            _ => self.min + (self.max - self.min) * t,
        };
        self.clamp(value)
    }

    /// Formats a value for display, including its unit suffix.
    pub fn format(&self, value: f32) -> String {
        let value = self.clamp(value);
        match self.unit {
            ParamUnit::Plain => format_significant(value),
            ParamUnit::Decibels => {
                if value <= self.min && self.min <= -60.0 {
                    "-inf dB".to_string()
                } else {
                    format!("{value:+.1} dB")
                }
            }
            ParamUnit::Hertz => {
                if value >= 1000.0 {
                    format!("{:.2} kHz", value / 1000.0)
                } else {
                    format!("{value:.0} Hz")
                }
            }
            ParamUnit::Seconds => format!("{value:.3} s"),
            ParamUnit::Milliseconds => format!("{value:.1} ms"),
            ParamUnit::Percent => format!("{:.0} %", value * 100.0),
            ParamUnit::Pan => match value {
                v if v.abs() < 0.005 => "C".to_string(),
                v if v < 0.0 => format!("L{:.0}", -v * 100.0),
                v => format!("R{:.0}", v * 100.0),
            },
            ParamUnit::Semitones => format!("{value:+.2} st"),
            ParamUnit::Ratio => format!("{value:.1}:1"),
            ParamUnit::Bpm => format!("{value:.2} BPM"),
            ParamUnit::Choice => self
                .choices
                .get(value.round().max(0.0) as usize)
                .map(|c| c.to_string())
                .unwrap_or_else(|| format_significant(value)),
            ParamUnit::Toggle => if value >= 0.5 { "On" } else { "Off" }.to_string(),
        }
    }
}

fn format_significant(value: f32) -> String {
    if value.abs() >= 100.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// Converts decibels to a linear gain multiplier. Values at or below -120 dB become silence.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -120.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Converts a linear gain multiplier to decibels, flooring at -120 dB.
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1e-6 {
        -120.0
    } else {
        20.0 * gain.abs().log10()
    }
}

/// Constant-power stereo pan law.
///
/// Returns `(left_gain, right_gain)` for `pan` in `-1.0` (hard left) to `1.0` (hard right).
/// Both gains are `1/sqrt(2)` at centre, so the perceived loudness stays constant as a source
/// sweeps across the image — a linear law would dip by 3 dB in the middle.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    // The angle sweeps 0..PI/2 as pan sweeps -1..1, so cos/sin trace a quarter circle.
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (angle.cos(), angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_normalisation_round_trips() {
        let descriptor = ParamDescriptor::new(0u32, "gain", "Gain", -60.0, 12.0, 0.0);
        for value in [-60.0, -24.0, 0.0, 6.0, 12.0] {
            let normalized = descriptor.normalize(value);
            assert!((descriptor.denormalize(normalized) - value).abs() < 1e-3);
        }
    }

    #[test]
    fn logarithmic_normalisation_round_trips() {
        let descriptor = ParamDescriptor::hertz(0u32, "freq", "Frequency", 20.0, 20_000.0, 1_000.0);
        for value in [20.0, 100.0, 1_000.0, 20_000.0] {
            let normalized = descriptor.normalize(value);
            assert!((descriptor.denormalize(normalized) / value - 1.0).abs() < 1e-4);
        }
        // Halfway along the control should be the geometric mean, not the arithmetic one.
        let midpoint = descriptor.denormalize(0.5);
        assert!((midpoint - 632.45).abs() < 1.0);
    }

    #[test]
    fn toggle_snaps_to_its_two_positions() {
        let descriptor = ParamDescriptor::toggle(0u32, "on", "On", false);
        assert_eq!(descriptor.clamp(0.4), 0.0);
        assert_eq!(descriptor.clamp(0.6), 1.0);
        assert_eq!(descriptor.format(1.0), "On");
    }

    #[test]
    fn decibel_conversions_agree() {
        assert!((db_to_gain(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-4);
        assert!((gain_to_db(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(db_to_gain(-140.0), 0.0);
    }

    #[test]
    fn pan_law_is_constant_power() {
        let (left, right) = pan_gains(0.0);
        assert!((left - right).abs() < 1e-6);
        assert!((left * left + right * right - 1.0).abs() < 1e-6);

        let (left, right) = pan_gains(-1.0);
        assert!((left - 1.0).abs() < 1e-6);
        assert!(right.abs() < 1e-6);

        let (left, right) = pan_gains(1.0);
        assert!(left.abs() < 1e-6);
        assert!((right - 1.0).abs() < 1e-6);
    }

    #[test]
    fn clamp_rejects_non_finite() {
        let descriptor = ParamDescriptor::new(0u32, "x", "X", 0.0, 1.0, 0.25);
        assert_eq!(descriptor.clamp(f32::NAN), 0.25);
    }
}
