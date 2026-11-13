//! Shared coordinates and ranking for desktop and agent sound discovery.

pub use auris_dsp::MeasuredEnvelope;
pub use auris_dsp::timbre::TimbreCharacter;

/// Where a measured sound sits on the sound map, each from 0 to 1.
///
/// The map and the agent's numeric targets share these, so a target of `attack: 0.9` means the
/// same corner of the map a person sees. Brightness and noise come from the reference notes'
/// spectra; attack and sustain from the envelope measured on real notes (C2, C4, C6), falling
/// back to the reference notes' own estimates only where no envelope could be measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct SoundAxes {
    /// Log spectral centroid, dark (0) to bright (1).
    pub brightness: f64,
    /// Attack time on a log scale from 10 s (0) to 1 ms (1): slow to immediate.
    pub attack: f64,
    /// Held level from −60 dB or below (0) to the peak (1): fading to holding.
    pub sustain: f64,
    /// Tonal (0) to noise-like (1).
    pub noise: f64,
}

impl SoundAxes {
    /// The axes of one sound, from its reference-note descriptors and its measured envelope.
    pub fn new(character: TimbreCharacter, envelope: Option<&MeasuredEnvelope>) -> Self {
        let (attack, sustain) = envelope.map_or((character.attack, character.sustain), |e| {
            (
                1. - (e.attack_ms.max(1.).log10() / 4.).clamp(0., 1.),
                (e.sustain_db / 60. + 1.).clamp(0., 1.),
            )
        });
        Self {
            brightness: character.brightness.clamp(0., 1.),
            attack,
            sustain,
            noise: character.noise.clamp(0., 1.),
        }
    }

    /// Brightness, attack and sustain as map coordinates from −1 to +1.
    pub fn position(self) -> [f64; 3] {
        [self.brightness, self.attack, self.sustain].map(|v| v * 2. - 1.)
    }

    fn values(self) -> [f64; 4] {
        [self.brightness, self.attack, self.sustain, self.noise]
    }
}

/// Optional targets on the sound map's axes, each 0 to 1; omitted axes do not affect ranking.
/// Values describe measured acoustics, not probabilities or musical quality.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimbreTargets {
    /// Dark (0) to bright (1).
    #[schemars(range(min = 0, max = 1))]
    pub brightness: Option<f64>,
    /// Slow swell (0) to immediate (1) attack; 0.5 is about 100 ms.
    #[schemars(range(min = 0, max = 1))]
    pub attack: Option<f64>,
    /// Fading (0, −60 dB or below) to holding at full level (1) while the key is held.
    #[schemars(range(min = 0, max = 1))]
    pub sustain: Option<f64>,
    /// Tonal (0) to noise-like (1).
    #[schemars(range(min = 0, max = 1))]
    pub noise: Option<f64>,
}

impl TimbreTargets {
    fn values(&self) -> [Option<f64>; 4] {
        [self.brightness, self.attack, self.sustain, self.noise]
    }

    /// Whether any target is set.
    pub fn is_empty(&self) -> bool {
        self.values().iter().all(Option::is_none)
    }

    /// Rejects values that are not finite numbers from 0 to 1.
    pub fn validate(&self) -> Result<(), String> {
        if self
            .values()
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
        {
            return Err("Timbre targets must be finite values between 0 and 1".into());
        }
        Ok(())
    }

    /// Mean squared target error; smaller is closer. Never a probability.
    pub fn distance(&self, axes: SoundAxes) -> f64 {
        let mut sum = 0.;
        let mut count = 0;
        for (target, actual) in self.values().into_iter().zip(axes.values()) {
            if let Some(target) = target {
                sum += (target - actual).powi(2);
                count += 1;
            }
        }
        sum / count.max(1) as f64
    }
}

/// An inclusive range; an omitted bound is open.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    /// Lowest accepted value.
    pub min: Option<f64>,
    /// Highest accepted value.
    pub max: Option<f64>,
}

impl Bounds {
    fn is_set(&self) -> bool {
        self.min.is_some() || self.max.is_some()
    }

    /// How far `value` lies outside, in the caller's units; zero inside.
    fn excess(&self, value: f64, scale: impl Fn(f64) -> f64) -> f64 {
        let below = self
            .min
            .map_or(0., |min| (scale(min) - scale(value)).max(0.));
        let above = self
            .max
            .map_or(0., |max| (scale(value) - scale(max)).max(0.));
        below + above
    }
}

/// Ranges on the measured amplitude envelope: times in milliseconds, sustain in dB below the
/// note's peak. Omitted fields do not affect ranking.
///
/// A sound outside a range is ranked by how far outside, never dropped, so a search that is too
/// strict still answers with the nearest sounds. Times compare on a log scale (8× off counts as
/// 1), sustain linearly (24 dB off counts as 1).
#[derive(
    Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeTargets {
    /// Time to reach the peak, e.g. `{"max": 10}` for a hard attack, `{"min": 200}` for a swell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack_ms: Option<Bounds>,
    /// Time from the peak to the sustain level, or to −60 dB for a sound that never sustains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay_ms: Option<Bounds>,
    /// Held level below the peak in dB, 0 to −120; `{"min": -6}` keeps sounds that hold up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sustain_db: Option<Bounds>,
    /// Time after note-off to fall 60 dB, e.g. `{"min": 1000}` for a long tail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ms: Option<Bounds>,
}

impl EnvelopeTargets {
    /// Whether any range is set.
    pub fn is_empty(&self) -> bool {
        [
            self.attack_ms,
            self.decay_ms,
            self.sustain_db,
            self.release_ms,
        ]
        .iter()
        .all(|b| !b.is_some_and(|b| b.is_set()))
    }

    /// Rejects non-finite bounds and a minimum above its maximum.
    pub fn validate(&self) -> Result<(), String> {
        for (name, bounds) in [
            ("attack_ms", self.attack_ms),
            ("decay_ms", self.decay_ms),
            ("sustain_db", self.sustain_db),
            ("release_ms", self.release_ms),
        ] {
            let Some(b) = bounds else { continue };
            if [b.min, b.max].into_iter().flatten().any(|v| !v.is_finite()) {
                return Err(format!("envelope.{name} bounds must be finite"));
            }
            if let (Some(min), Some(max)) = (b.min, b.max)
                && min > max
            {
                return Err(format!("envelope.{name} min must not exceed max"));
            }
        }
        Ok(())
    }

    /// Mean range excess; zero when the envelope meets every range.
    pub fn distance(&self, envelope: &MeasuredEnvelope) -> f64 {
        let time = |ms: f64| ms.max(1.).log2() / 3.;
        let level = |db: f64| db / 24.;
        let mut sum = 0.;
        let mut count = 0;
        for (bounds, value, scale) in [
            (
                self.attack_ms,
                envelope.attack_ms,
                &time as &dyn Fn(f64) -> f64,
            ),
            (self.decay_ms, envelope.decay_ms, &time),
            (self.sustain_db, envelope.sustain_db, &level),
            (self.release_ms, envelope.release_ms, &time),
        ] {
            if let Some(bounds) = bounds.filter(Bounds::is_set) {
                sum += bounds.excess(value, scale);
                count += 1;
            }
        }
        sum / count.max(1) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(attack_ms: f64, sustain_db: f64, release_ms: f64) -> MeasuredEnvelope {
        MeasuredEnvelope {
            attack_ms,
            decay_ms: 100.,
            sustain_db,
            release_ms,
            decaying: false,
            decay_estimated: false,
            release_estimated: false,
        }
    }

    #[test]
    fn envelope_ranges_rank_by_how_far_outside_and_never_by_inside() {
        let targets = EnvelopeTargets {
            attack_ms: Some(Bounds {
                min: None,
                max: Some(20.),
            }),
            release_ms: Some(Bounds {
                min: Some(300.),
                max: Some(800.),
            }),
            ..Default::default()
        };
        assert_eq!(targets.distance(&envelope(5., 0., 500.)), 0.);
        assert_eq!(targets.distance(&envelope(19., -40., 799.)), 0.);
        // 160 ms is eight times the limit: one unit, averaged over two ranges.
        assert!((targets.distance(&envelope(160., 0., 500.)) - 0.5).abs() < 1e-9);
        let near = targets.distance(&envelope(5., 0., 1000.));
        let far = targets.distance(&envelope(5., 0., 4000.));
        assert!(0. < near && near < far);
        assert!(!targets.is_empty());
        assert!(EnvelopeTargets::default().is_empty());
        let backwards = EnvelopeTargets {
            sustain_db: Some(Bounds {
                min: Some(-3.),
                max: Some(-9.),
            }),
            ..Default::default()
        };
        assert!(backwards.validate().is_err());
    }

    #[test]
    fn axes_come_from_the_measured_envelope_and_fall_back_to_reference_notes() {
        let character = TimbreCharacter {
            brightness: 0.7,
            attack: 0.2,
            sustain: 0.3,
            noise: 0.1,
            ..Default::default()
        };
        // 1 ms is immediate, 10 s the slowest; 100 ms sits halfway on the log scale.
        let immediate = SoundAxes::new(character, Some(&envelope(1., 0., 100.)));
        assert_eq!((immediate.attack, immediate.sustain), (1., 1.));
        let swell = SoundAxes::new(character, Some(&envelope(100., -30., 100.)));
        assert!((swell.attack - 0.5).abs() < 1e-9 && (swell.sustain - 0.5).abs() < 1e-9);
        let faded = SoundAxes::new(character, Some(&envelope(20_000., -90., 100.)));
        assert_eq!((faded.attack, faded.sustain), (0., 0.));
        assert_eq!(faded.position()[1..], [-1., -1.]);
        let unmeasured = SoundAxes::new(character, None);
        assert_eq!((unmeasured.attack, unmeasured.sustain), (0.2, 0.3));
        assert_eq!((unmeasured.brightness, unmeasured.noise), (0.7, 0.1));
        assert!((unmeasured.position()[0] - 0.4).abs() < 1e-9);
    }

    #[test]
    fn targets_rank_by_the_map_axes_and_reject_values_off_the_scale() {
        let pad = SoundAxes {
            brightness: 0.25,
            attack: 0.3,
            sustain: 0.8,
            noise: 0.1,
        };
        let pluck = SoundAxes {
            brightness: 0.8,
            attack: 1.,
            sustain: 0.,
            noise: 0.,
        };
        let soft = TimbreTargets {
            attack: Some(0.3),
            sustain: Some(0.8),
            ..Default::default()
        };
        assert_eq!(soft.distance(pad), 0.);
        assert!(soft.distance(pad) < soft.distance(pluck));
        assert!(TimbreTargets::default().is_empty() && !soft.is_empty());
        for bad in [f64::NAN, -0.1, 1.1] {
            let targets = TimbreTargets {
                noise: Some(bad),
                ..Default::default()
            };
            assert!(targets.validate().is_err());
        }
        assert!(soft.validate().is_ok());
    }
}
