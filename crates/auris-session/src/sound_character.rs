//! Shared interpretation and ranking for desktop and agent sound discovery.

pub use auris_dsp::MeasuredEnvelope;
pub use auris_dsp::timbre::TimbreCharacter;

/// Optional fixed-scale targets; omitted dimensions do not affect ranking.
/// Values describe reference-note acoustics, not probabilities or measured musical quality.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimbreTargets {
    /// Dark (0) to bright (1).
    #[schemars(range(min = 0, max = 1))]
    pub brightness: Option<f64>,
    /// Slow (0) to immediate (1) attack.
    #[schemars(range(min = 0, max = 1))]
    pub attack: Option<f64>,
    /// Decaying (0) to sustained (1) held note.
    #[schemars(range(min = 0, max = 1))]
    pub sustain: Option<f64>,
    /// Short (0) to at least the captured 400 ms (1) release.
    #[schemars(range(min = 0, max = 1))]
    pub release: Option<f64>,
    /// Tonal (0) to noise-like (1).
    #[schemars(range(min = 0, max = 1))]
    pub noise: Option<f64>,
}

impl TimbreTargets {
    /// Brightness, attack speed, sustain, captured release, noise, in descriptor order.
    pub fn values(&self) -> [Option<f64>; 5] {
        [
            self.brightness,
            self.attack,
            self.sustain,
            self.release,
            self.noise,
        ]
    }

    /// Interpret supported Japanese/English impressions; explicit targets take precedence.
    /// Returns recognized terms so callers can distinguish them from uninterpreted prose.
    pub fn interpret(query: &str, explicit: &Self) -> Result<(Self, Vec<String>), String> {
        let (targets, recognized) = Self::interpret_partial(query, explicit)?;
        if targets.values().iter().all(Option::is_none) {
            return Err("No supported impression found; use text search for names or supply explicit timbre targets".into());
        }
        Ok((targets, recognized))
    }

    /// [`interpret`](Self::interpret) without requiring any target, for a search whose
    /// constraints are all on the measured envelope.
    pub fn interpret_partial(query: &str, explicit: &Self) -> Result<(Self, Vec<String>), String> {
        if query.len() > 1024 {
            return Err("Timbre query must be at most 1024 UTF-8 bytes".into());
        }
        let query = query.to_lowercase();
        if ["ない", "なく", "ではなく", "not ", "without ", "less "]
            .iter()
            .any(|word| query.contains(word))
        {
            return Err("Use explicit targets for negation/comparisons; the local impression dictionary does not interpret them".into());
        }
        let mut sums = [0.; 5];
        let mut counts = [0.; 5];
        let mut recognized = Vec::new();
        // Compound impressions are intentionally acoustic hypotheses that can be overridden.
        for (term_index, (words, values)) in [
            (
                "ふんわり|ふわふわ|柔らか|やわらか|soft|fluffy",
                [Some(0.25), Some(0.3), Some(0.8), None, Some(0.1)],
            ),
            (
                "ポコポコ|ぽこぽこ|ぽこん|plucky|plucked",
                [Some(0.3), Some(1.), Some(0.1), Some(0.1), Some(0.)],
            ),
            (
                "キラキラ|きらきら|煌びやか|sparkling|shimmering",
                [Some(0.85), None, None, Some(0.8), Some(0.)],
            ),
            (
                "ザラザラ|ざらざら|ざらつ|rough|gritty|noisy",
                [None, None, None, None, Some(0.8)],
            ),
            ("明る|bright", [Some(0.9), None, None, None, None]),
            (
                "暗い|暗め|dark|mellow|丸い|丸め",
                [Some(0.15), None, None, None, None],
            ),
            (
                "鋭い|鋭く|硬い|硬め|sharp|hard|パンチ|punchy",
                [Some(0.75), Some(1.), None, None, None],
            ),
            (
                "ゆっくり|緩やか|slow attack|slow-attack",
                [None, Some(0.15), None, None, None],
            ),
            (
                "持続|伸びる|sustained|sustaining",
                [None, None, Some(0.95), None, None],
            ),
            (
                "短い|短め|歯切れ|short|dry",
                [None, None, Some(0.15), Some(0.1), None],
            ),
            (
                "長い余韻|余韻が長|余韻は長|長め|long release|ringing",
                [None, None, None, Some(0.95), None],
            ),
            (
                "澄んだ|クリア|clear|pure|ピュア",
                [None, None, None, None, Some(0.)],
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(word) = words.split('|').find(|word| {
                if word.is_ascii() {
                    query.match_indices(word).any(|(i, _)| {
                        !query[..i]
                            .chars()
                            .next_back()
                            .is_some_and(|c| c.is_ascii_alphabetic())
                            && !query[i + word.len()..]
                                .chars()
                                .next()
                                .is_some_and(|c| c.is_ascii_alphabetic())
                    })
                } else {
                    query.contains(word)
                }
            }) {
                recognized.push(word.to_owned());
                for i in 0..5 {
                    if let Some(value) = values[i] {
                        if term_index >= 4 {
                            sums[i] = value;
                            counts[i] = 1.;
                        } else {
                            sums[i] += value;
                            counts[i] += 1.;
                        }
                    }
                }
            }
        }
        let explicit_values = explicit.values();
        let values: [Option<f64>; 5] = std::array::from_fn(|i| {
            explicit_values[i].or_else(|| (counts[i] > 0.).then(|| sums[i] / counts[i]))
        });
        if values
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
        {
            return Err("Timbre targets must be finite values between 0 and 1".into());
        }
        Ok((
            Self {
                brightness: values[0],
                attack: values[1],
                sustain: values[2],
                release: values[3],
                noise: values[4],
            },
            recognized,
        ))
    }

    /// Mean squared target error; smaller is closer. Never a probability.
    pub fn distance(&self, character: TimbreCharacter) -> f64 {
        let mut sum = 0.;
        let mut count = 0;
        for (target, actual) in self.values().into_iter().zip(character.values()) {
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
    fn impressions_rank_acoustics_and_explicit_targets_override_dictionary() {
        let (soft, words) =
            TimbreTargets::interpret("ふんわりした音", &TimbreTargets::default()).unwrap();
        assert_eq!(words, ["ふんわり"]);
        let pad = TimbreCharacter {
            brightness: 0.25,
            attack: 0.3,
            sustain: 0.8,
            noise: 0.1,
            ..Default::default()
        };
        let pluck = TimbreCharacter {
            brightness: 0.8,
            attack: 1.,
            sustain: 0.,
            ..Default::default()
        };
        assert!(soft.distance(pad) < soft.distance(pluck));
        let (short, _) = TimbreTargets::interpret(
            "ふんわり、余韻は短め",
            &TimbreTargets {
                release: Some(0.05),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(short.release, Some(0.05));
    }

    #[test]
    fn unknown_negation_and_invalid_targets_are_explicit_errors() {
        for query in ["microsoft", "明るくない", "not bright", "宇宙"] {
            assert!(TimbreTargets::interpret(query, &TimbreTargets::default()).is_err());
        }
        assert!(
            TimbreTargets::interpret(
                "",
                &TimbreTargets {
                    noise: Some(f64::NAN),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
