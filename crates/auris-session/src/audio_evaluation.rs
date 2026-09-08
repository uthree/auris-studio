//! Replaceable objectives that evaluate rendered PCM, independently of search and editing.
//!
//! Reference matching is one implementation. A future learned audio/text CLAP evaluator can own
//! its model and fixed text embedding behind the same [`AudioEvaluator`] trait; the optimizer
//! still supplies real rendered audio and consumes finite fitness plus named diagnostics.

use auris_core::AudioBuffer;
use auris_dsp::reference_features::ReferenceFeatures;

const REFERENCE_DISTANCE_STEPS: f64 = 1_000_000.0;

/// One finite diagnostic in an evaluator's documented units.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioMetric {
    /// Stable name of the measurement.
    pub name: String,
    /// Finite raw value; its units depend on the evaluator.
    pub value: f64,
}

/// A higher-is-better fitness and the measurements explaining it.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioEvaluation {
    /// Finite objective value; larger values win, with earlier candidates retained on ties.
    pub fitness: f64,
    /// Raw diagnostics, not probabilities or an implied universal quality percentage.
    pub metrics: Vec<AudioMetric>,
}

impl AudioEvaluation {
    /// Rejects non-finite fitness or diagnostics before feedback, ranking, or result adoption.
    pub fn validate(&self) -> Result<(), String> {
        if !self.fitness.is_finite() {
            return Err("audio evaluator fitness must be finite".into());
        }
        if let Some(metric) = self.metrics.iter().find(|metric| !metric.value.is_finite()) {
            return Err(format!(
                "audio evaluator metric `{}` must be finite",
                metric.name
            ));
        }
        Ok(())
    }
}

/// A fixed objective over actual rendered audio, replaceable without changing the optimizer.
///
/// Keep all reference features, model weights, prompts and settings constant for one search.
/// Evaluation runs on a worker. Implementations must not modify the current document.
pub trait AudioEvaluator: Send + Sync {
    /// Measures one candidate's complete rendered PCM and returns its ranking and diagnostics.
    fn evaluate(&self, audio: &AudioBuffer) -> Result<AudioEvaluation, String>;

    /// A human-readable description of the fixed objective and its scale.
    fn description(&self) -> String;
}

/// Matches a mix's tonal balance, dynamics, stereo and transient statistics to reference PCM.
///
/// Shared output gain does not improve the score. This compares acoustic characteristics rather
/// than melody, arrangement, semantic mood, or a sample-aligned reconstruction of the reference.
/// Its fitness is the negative weighted feature distance rounded to the nearest 0.000001,
/// never a quality percentage. Ranking ignores smaller PCM roundoff differences; diagnostic
/// metrics retain the unrounded distances. Other evaluators keep their own units and precision.
#[derive(Clone, Debug)]
pub struct ReferenceAudioEvaluator {
    reference: ReferenceFeatures,
}

impl ReferenceAudioEvaluator {
    /// Captures fixed features from finite, non-silent mono/stereo reference audio.
    pub fn new(reference: &AudioBuffer) -> Result<Self, String> {
        Ok(Self {
            reference: ReferenceFeatures::analyze(reference).map_err(str::to_owned)?,
        })
    }
}

impl AudioEvaluator for ReferenceAudioEvaluator {
    fn evaluate(&self, audio: &AudioBuffer) -> Result<AudioEvaluation, String> {
        let features = ReferenceFeatures::analyze(audio).map_err(str::to_owned)?;
        let distance = features.distance(&self.reference);
        let result = AudioEvaluation {
            // Shared gain is normalized by analysis, but f32 rendering still introduces tiny
            // rounding differences. Do not adopt fader changes on the strength of that noise.
            fitness: -(distance.total() * REFERENCE_DISTANCE_STEPS).round()
                / REFERENCE_DISTANCE_STEPS,
            metrics: [
                ("reference_distance", distance.total()),
                ("reference_spectrum_distance", distance.spectrum),
                ("reference_dynamics_distance", distance.dynamics),
                ("reference_stereo_distance", distance.stereo),
                ("reference_rhythm_distance", distance.rhythm),
            ]
            .into_iter()
            .map(|(name, value)| AudioMetric {
                name: name.into(),
                value,
            })
            .collect(),
        };
        result.validate()?;
        Ok(result)
    }

    fn description(&self) -> String {
        "Reference audio: spectrum, dynamics, stereo and transient distance; lower distance is closer".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64) -> AudioBuffer {
        let channel = (0..24_000)
            .map(|sample| {
                (0.2 * (std::f64::consts::TAU * hz * sample as f64 / 24_000.0).sin()) as f32
            })
            .collect();
        AudioBuffer::from_planar(vec![channel], 24_000.0).unwrap()
    }

    #[test]
    fn reference_evaluator_ranks_the_target_above_wrong_audio_through_the_trait() {
        let reference = tone(220.0);
        let evaluator: Box<dyn AudioEvaluator> =
            Box::new(ReferenceAudioEvaluator::new(&reference).unwrap());
        let target = evaluator.evaluate(&reference).unwrap();
        let wrong = evaluator.evaluate(&tone(5000.0)).unwrap();
        assert_eq!(target.fitness, 0.0);
        assert!(target.fitness > wrong.fitness);
        assert_eq!(target.metrics.len(), 5);
        assert!(target.metrics.iter().all(|metric| metric.value == 0.0));
        assert!(
            wrong
                .metrics
                .iter()
                .all(|metric| (0.0..=1.0).contains(&metric.value))
        );
        target.validate().unwrap();
        wrong.validate().unwrap();
    }

    #[test]
    fn reference_fitness_ties_shared_gain_despite_unrounded_pcm_diagnostics() {
        let reference = tone(220.0);
        let evaluator = ReferenceAudioEvaluator::new(&reference).unwrap();
        let baseline = evaluator.evaluate(&reference).unwrap();
        let mut observed_roundoff = false;
        for gain in [0.01, -0.4, 3.0] {
            let mut scaled = reference.clone();
            for sample in scaled.channels_mut().iter_mut().flatten() {
                *sample *= gain;
            }
            let evaluation = evaluator.evaluate(&scaled).unwrap();
            assert_eq!(evaluation.fitness, baseline.fitness, "gain {gain}");
            let raw_distance = evaluation
                .metrics
                .iter()
                .find(|metric| metric.name == "reference_distance")
                .unwrap()
                .value;
            assert!(raw_distance < 1e-6, "gain {gain}: {raw_distance}");
            observed_roundoff |= raw_distance > 0.0;
        }
        assert!(observed_roundoff, "diagnostics must retain PCM roundoff");
    }

    #[test]
    fn every_nonfinite_fitness_or_metric_is_refused() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                AudioEvaluation {
                    fitness: invalid,
                    metrics: Vec::new()
                }
                .validate()
                .is_err()
            );
            assert!(
                AudioEvaluation {
                    fitness: 0.0,
                    metrics: vec![AudioMetric {
                        name: "broken".into(),
                        value: invalid,
                    }]
                }
                .validate()
                .is_err()
            );
        }
    }
}
