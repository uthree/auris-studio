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

use crate::project::{EffectSlotId, SendId, TrackId};

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

    /// The range to read [`Self::min`] and [`Self::max`] as.
    ///
    /// Not simply the two fields. They are public, on a type that is also deserialised, and for a
    /// hosted CLAP plugin they are numbers the *plugin* chose — and [`f32::clamp`] does not
    /// tolerate bounds the wrong way round or bounds that are not numbers: it asserts, in release
    /// builds as well as debug ones. A parameter is clamped on the audio callback thread, where
    /// nothing is watching for a panic, so reading the fields directly puts a third party's
    /// arithmetic mistake in a position to end the process mid-block.
    ///
    /// A range like that describes nothing to clamp against, so it is read as the unit range
    /// instead. A knob in the wrong place is a bug somebody can find and fix; silence is not.
    fn range(&self) -> (f32, f32) {
        match (self.min, self.max) {
            (min, max) if min.is_finite() && max.is_finite() && min <= max => (min, max),
            _ => (0.0, 1.0),
        }
    }

    /// Clamps `value` into the descriptor's range, snapping to steps when discrete.
    ///
    /// Total: every descriptor has an answer for every `f32`, however either of them was built.
    /// The range is read through `Self::range`, whose own comment is why that matters.
    pub fn clamp(&self, value: f32) -> f32 {
        let (min, max) = self.range();
        let value = if value.is_finite() {
            value
        } else {
            self.default
        };
        // The default is a plugin's own number too, so it can be the thing that is not a number.
        let value = if value.is_finite() { value } else { min };
        let value = value.clamp(min, max);
        match self.steps {
            Some(steps) if steps > 1 => {
                let span = max - min;
                if span.abs() < f32::EPSILON {
                    min
                } else {
                    let quantum = span / (steps - 1) as f32;
                    min + ((value - min) / quantum).round() * quantum
                }
            }
            _ => value,
        }
    }

    /// Maps a real value onto a control's 0..1 travel.
    pub fn normalize(&self, value: f32) -> f32 {
        let (min, max) = self.range();
        let value = self.clamp(value);
        match self.curve {
            ParamValueCurve::Logarithmic if min > 0.0 && max > 0.0 => {
                let span = (max / min).ln();
                if span.abs() < f32::EPSILON {
                    0.0
                } else {
                    ((value / min).ln() / span).clamp(0.0, 1.0)
                }
            }
            ParamValueCurve::Power(exponent) if exponent > 0.0 => {
                let span = max - min;
                if span.abs() < f32::EPSILON {
                    0.0
                } else {
                    ((value - min) / span).powf(1.0 / exponent).clamp(0.0, 1.0)
                }
            }
            _ => {
                let span = max - min;
                if span.abs() < f32::EPSILON {
                    0.0
                } else {
                    ((value - min) / span).clamp(0.0, 1.0)
                }
            }
        }
    }

    /// Inverse of [`Self::normalize`].
    pub fn denormalize(&self, normalized: f32) -> f32 {
        let (min, max) = self.range();
        let t = normalized.clamp(0.0, 1.0);
        let value = match self.curve {
            ParamValueCurve::Logarithmic if min > 0.0 && max > 0.0 => min * (max / min).powf(t),
            ParamValueCurve::Power(exponent) if exponent > 0.0 => {
                min + (max - min) * t.powf(exponent)
            }
            _ => min + (max - min) * t,
        };
        self.clamp(value)
    }

    /// Formats a value for display, including its unit suffix.
    pub fn format(&self, value: f32) -> String {
        let (min, _) = self.range();
        let value = self.clamp(value);
        match self.unit {
            ParamUnit::Plain => format_significant(value),
            ParamUnit::Decibels => {
                if value <= min && min <= -60.0 {
                    "-inf dB".to_string()
                } else {
                    format!("{value:+.1} dB")
                }
            }
            ParamUnit::Hertz => {
                if value >= 1000.0 {
                    format!("{:.2} kHz", value / 1000.0)
                } else if value < 20.0 {
                    // Below hearing, so this is a modulation rate rather than a pitch or a filter
                    // corner — and there the whole of the useful range is one decade wide. Rounded
                    // to a whole number, a vibrato at 5.5 Hz and one at 6.4 Hz read the same.
                    format!("{value:.1} Hz")
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

/// A parameter anywhere in the project, named so it can be read, written or automated.
///
/// Routing every parameter edit through one enum means the "update the document, then tell the
/// audio thread" step exists in exactly one place. Without it each control reimplements half of
/// it, and eventually one of them forgets the second half and the document silently disagrees
/// with what is heard.
///
/// It lives here rather than in `auris-session`, where it was written, because the document now
/// holds automation and an automation lane is a target and a shape — and the document model may
/// not name a crate above it. `auris_session::param` re-exports this module, so
/// `ParamTarget` still resolves by its old path.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParamTarget {
    /// A track's fader, in decibels.
    TrackGain(TrackId),
    /// A track's stereo position, -1.0 to 1.0.
    TrackPan(TrackId),
    /// The master fader, in decibels.
    MasterGain,
    /// The master bus stereo position.
    MasterPan,
    /// How much of a track one of its sends carries, in decibels.
    Send {
        /// Track the send is taken from.
        track: TrackId,
        /// Which send on that track.
        send: SendId,
    },
    /// A parameter of a track's instrument.
    Instrument {
        /// Track whose instrument is addressed.
        track: TrackId,
        /// Index of the parameter within that instrument.
        param: ParamId,
    },
    /// A parameter of an effect, on a track or on the master bus.
    Effect {
        /// Track the effect sits on; `None` means the master bus.
        track: Option<TrackId>,
        /// Which slot in the chain.
        slot: EffectSlotId,
        /// Index of the parameter within that effect.
        param: ParamId,
    },
}

impl ParamTarget {
    /// The track this target belongs to, if any.
    ///
    /// `None` covers both the master bus and a target that is not track-scoped.
    pub fn track(self) -> Option<TrackId> {
        match self {
            ParamTarget::TrackGain(id) | ParamTarget::TrackPan(id) => Some(id),
            ParamTarget::Send { track, .. } => Some(track),
            ParamTarget::Instrument { track, .. } => Some(track),
            ParamTarget::Effect { track, .. } => track,
            ParamTarget::MasterGain | ParamTarget::MasterPan => None,
        }
    }

    /// `true` for the mixer's own controls rather than a plugin's.
    ///
    /// These have no [`ParamDescriptor`] of their own, so the session synthesises one — see
    /// `auris_session::Session::descriptor_for`.
    pub fn is_builtin(self) -> bool {
        matches!(
            self,
            ParamTarget::TrackGain(_)
                | ParamTarget::TrackPan(_)
                | ParamTarget::MasterGain
                | ParamTarget::MasterPan
                | ParamTarget::Send { .. }
        )
    }

    /// Whether this target names anything on `track`.
    ///
    /// What a deleted track asks of the automation: every lane pointing into it has to go with
    /// it, or the document keeps curves addressed to something that no longer exists.
    pub fn belongs_to(self, track: TrackId) -> bool {
        self.track() == Some(track)
    }

    /// Which parameter of a plugin this target names, for the two variants that name one.
    ///
    /// `None` for the mixer's own controls, which are not positions in anybody's list.
    pub fn param(self) -> Option<ParamId> {
        match self {
            ParamTarget::Instrument { param, .. } | ParamTarget::Effect { param, .. } => {
                Some(param)
            }
            _ => None,
        }
    }

    /// The same target with its plugin parameter re-pointed; unchanged for a mixer control.
    ///
    /// What a plugin that has reordered its list asks of a saved automation lane — see
    /// [`Automation::realign_by_key`](crate::automation::Automation::realign_by_key).
    pub fn with_param(self, param: ParamId) -> Self {
        match self {
            ParamTarget::Instrument { track, .. } => ParamTarget::Instrument { track, param },
            ParamTarget::Effect { track, slot, .. } => ParamTarget::Effect { track, slot, param },
            other => other,
        }
    }
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
    fn a_range_the_wrong_way_round_is_read_as_the_unit_range() {
        // A hosted plugin's range is the plugin's own arithmetic, and `f32::clamp` asserts on
        // bounds it cannot order — in release builds too. This is clamped on the audio callback
        // thread, so the assert would end the process mid-block rather than misplace a knob.
        let mut descriptor =
            ParamDescriptor::new(0u32, "cutoff", "Cutoff", 20.0, 20_000.0, 1_000.0);
        descriptor.min = 10.0;
        descriptor.max = 0.0;

        assert_eq!(descriptor.clamp(5.0), 1.0);
        assert_eq!(descriptor.clamp(-3.0), 0.0);
        assert_eq!(descriptor.normalize(0.25), 0.25);
        assert_eq!(descriptor.denormalize(0.75), 0.75);
    }

    #[test]
    fn a_range_that_is_not_a_number_is_read_as_the_unit_range() {
        for (min, max) in [
            (f32::NAN, 1.0),
            (0.0, f32::NAN),
            (f32::NEG_INFINITY, f32::INFINITY),
        ] {
            let mut descriptor = ParamDescriptor::new(0u32, "p", "P", 0.0, 1.0, 0.5);
            descriptor.min = min;
            descriptor.max = max;
            assert_eq!(descriptor.clamp(2.0), 1.0, "{min}..{max}");
            assert_eq!(descriptor.clamp(-2.0), 0.0, "{min}..{max}");
            assert!(descriptor.normalize(0.5).is_finite(), "{min}..{max}");
        }
    }

    #[test]
    fn a_default_that_is_not_a_number_does_not_come_back_out() {
        // `clamp` falls back to the default for a value it cannot use; a plugin that reports both
        // as not-a-number would otherwise have the fallback hand one straight back.
        let mut descriptor = ParamDescriptor::new(0u32, "p", "P", -6.0, 6.0, 0.0);
        descriptor.default = f32::NAN;
        assert_eq!(descriptor.clamp(f32::NAN), -6.0);
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

    #[test]
    fn the_master_bus_belongs_to_no_track() {
        assert_eq!(ParamTarget::MasterGain.track(), None);
        assert_eq!(
            ParamTarget::Effect {
                track: None,
                slot: EffectSlotId(3),
                param: ParamId(0)
            }
            .track(),
            None
        );
    }

    #[test]
    fn track_scoped_targets_report_their_track() {
        let id = TrackId(7);
        assert_eq!(ParamTarget::TrackGain(id).track(), Some(id));
        assert_eq!(ParamTarget::TrackPan(id).track(), Some(id));
        assert_eq!(
            ParamTarget::Instrument {
                track: id,
                param: ParamId(2)
            }
            .track(),
            Some(id)
        );
    }

    #[test]
    fn only_mixer_controls_are_builtin() {
        assert!(ParamTarget::MasterPan.is_builtin());
        assert!(ParamTarget::TrackGain(TrackId(1)).is_builtin());
        assert!(
            !ParamTarget::Instrument {
                track: TrackId(1),
                param: ParamId(0)
            }
            .is_builtin()
        );
    }

    #[test]
    fn a_target_round_trips_through_json() {
        // It is a document field now, so a shape serde cannot carry back is a project that opens
        // with its automation pointing somewhere else.
        for target in [
            ParamTarget::MasterPan,
            ParamTarget::TrackGain(TrackId(4)),
            ParamTarget::Instrument {
                track: TrackId(2),
                param: ParamId(5),
            },
            ParamTarget::Effect {
                track: Some(TrackId(1)),
                slot: EffectSlotId(9),
                param: ParamId(3),
            },
            ParamTarget::Effect {
                track: None,
                slot: EffectSlotId(2),
                param: ParamId(0),
            },
            ParamTarget::Send {
                track: TrackId(3),
                send: SendId(8),
            },
        ] {
            let json = serde_json::to_string(&target).expect("serialises");
            let back: ParamTarget = serde_json::from_str(&json).expect("deserialises");
            assert_eq!(back, target, "{json}");
        }
    }
}
