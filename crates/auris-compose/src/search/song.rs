//! Auris song parameters and the fixed written-note density objective.

use crate::rng::Rng;
use crate::{Composition, SongSpec};

use super::algorithms::{HillClimber, RandomSearch};
use super::{
    ComposeError, Composer, Evaluation, EvaluationError, Evaluator, Metric, SearchError,
    SearchResult, ValidationError, run_search,
};

/// Inclusive bounds and the largest step of a hill-climbing proposal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterBounds {
    /// Lowest permitted value, at least zero.
    pub min: f32,
    /// Highest permitted value, at most one and greater than `min`.
    pub max: f32,
    /// Positive step no larger than the range, and large enough to change an `f32` value.
    pub step: f32,
}

impl ParameterBounds {
    fn validate(self) -> Result<(), SearchError> {
        if !self.min.is_finite()
            || !self.max.is_finite()
            || self.min < 0.0
            || self.max > 1.0
            || self.min >= self.max
        {
            return Err(SearchError::InvalidRequest(
                "parameter bounds must be finite and satisfy 0 <= min < max <= 1".into(),
            ));
        }
        if !self.step.is_finite()
            || self.step <= 0.0
            || self.step > self.max - self.min
            || self.max - self.step >= self.max
            || self.min + self.step <= self.min
        {
            return Err(SearchError::InvalidRequest(
                "parameter step must be positive, no larger than the range, and able to change an f32 value".into(),
            ));
        }
        Ok(())
    }

    fn contains(self, value: f32) -> bool {
        value.is_finite() && (self.min..=self.max).contains(&value)
    }

    fn sample(self, rng: &mut Rng) -> f32 {
        (self.min + rng.unit() * (self.max - self.min)).clamp(self.min, self.max)
    }

    fn neighbor(self, value: f32, increase: bool) -> f32 {
        let step = if increase { self.step } else { -self.step };
        let proposed = (value + step).clamp(self.min, self.max);
        // An outward move at a boundary turns inward; an interior move stops at the boundary.
        // This consumes no extra randomness, so hitting an edge cannot shift future draws.
        if proposed == value {
            (value - step).clamp(self.min, self.max)
        } else {
            proposed
        }
    }
}

/// One existing part's explicit density dial to search.
#[derive(Clone, Debug, PartialEq)]
pub struct PartDensity {
    /// Stable part name in the base song's roster.
    pub part: String,
    /// Permitted density and mutation step.
    pub bounds: ParameterBounds,
}

/// One existing section's intensity dial to search.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionIntensity {
    /// Section name, which must occur in the song's form.
    pub section: String,
    /// Permitted intensity and mutation step.
    pub bounds: ParameterBounds,
}

/// At most two mutable existing composition parameters; every other field stays fixed.
///
/// At least one dial is required. A selected part must already have an explicit density within
/// its bounds, so the hill climber can evaluate the unchanged base first. Rhythms and section
/// overrides retain their normal precedence; they can make the selected dial ineffective.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchSpace {
    /// Optional density of one named part.
    pub part_density: Option<PartDensity>,
    /// Optional intensity of one named section.
    pub section_intensity: Option<SectionIntensity>,
}

impl SearchSpace {
    /// Checks bounds, selectors, and the base values without rewriting the specification.
    pub fn validate(&self, base: &SongSpec) -> Result<(), SearchError> {
        if self.part_density.is_none() && self.section_intensity.is_none() {
            return Err(SearchError::InvalidRequest(
                "the search space must select a part density or section intensity".into(),
            ));
        }
        if let Some(dial) = &self.part_density {
            dial.bounds.validate()?;
            let part = base.parts.iter().find(|part| part.name == dial.part);
            let Some(part) = part else {
                return Err(SearchError::InvalidRequest(format!(
                    "part `{}` does not exist in the base song",
                    dial.part
                )));
            };
            if !part
                .density
                .is_some_and(|value| dial.bounds.contains(value))
            {
                return Err(SearchError::InvalidRequest(format!(
                    "part `{}` needs an explicit density within its search bounds",
                    dial.part
                )));
            }
            if !base.form.iter().any(|name| {
                base.sections.get(name).is_some_and(|section| {
                    section.parts.is_empty() || section.parts.contains(&dial.part)
                })
            }) {
                return Err(SearchError::InvalidRequest(format!(
                    "part `{}` must play in at least one section of the form",
                    dial.part
                )));
            }
        }
        if let Some(dial) = &self.section_intensity {
            dial.bounds.validate()?;
            if !base.form.contains(&dial.section) {
                return Err(SearchError::InvalidRequest(format!(
                    "section `{}` does not occur in the base song's form",
                    dial.section
                )));
            }
            if !base
                .sections
                .get(&dial.section)
                .is_some_and(|section| dial.bounds.contains(section.intensity))
            {
                return Err(SearchError::InvalidRequest(format!(
                    "section `{}` needs an intensity within its search bounds",
                    dial.section
                )));
            }
        }
        Ok(())
    }

    pub(super) fn sample(&self, base: &SongSpec, rng: &mut Rng) -> SongSpec {
        let mut params = base.clone();
        if let Some(dial) = &self.part_density {
            let part = params.parts.iter_mut().find(|part| part.name == dial.part);
            if let Some(part) = part {
                part.density = Some(dial.bounds.sample(rng));
            }
        }
        if let Some(dial) = &self.section_intensity
            && let Some(section) = params.sections.get_mut(&dial.section)
        {
            section.intensity = dial.bounds.sample(rng);
        }
        params
    }

    pub(super) fn neighbor(&self, parent: &SongSpec, rng: &mut Rng) -> SongSpec {
        let mut params = parent.clone();
        let dimensions = usize::from(self.part_density.is_some())
            + usize::from(self.section_intensity.is_some());
        let dimension = rng.below(dimensions);
        let increase = rng.chance(0.5);
        if let Some(dial) = &self.part_density
            && dimension == 0
        {
            if let Some(part) = params.parts.iter_mut().find(|part| part.name == dial.part)
                && let Some(value) = part.density
            {
                part.density = Some(dial.bounds.neighbor(value, increase));
            }
        } else if let Some(dial) = &self.section_intensity
            && let Some(section) = params.sections.get_mut(&dial.section)
        {
            section.intensity = dial.bounds.neighbor(section.intensity, increase);
        }
        params
    }
}

/// The concrete proposal algorithm used by [`search_composition`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMethod {
    /// Independently sample every proposal within the bounds.
    Random,
    /// Start from the base, then mutate the strictly best evaluated incumbent.
    HillClimb,
}

/// A fixed target for the total number of written notes per bar.
///
/// Fitness is `-abs(notes_per_bar - target_notes_per_bar)`, maximized at zero. Every written
/// note counts once, including simultaneous chord tones and individual drum hits. The divisor
/// is the generated composition's full length in its meter, including an ending. Performance
/// transforms, rendered audio, and vocals later materialized by the session are not evaluated.
///
/// This measures arrangement density and distance from a requested target, not musical quality.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensityTarget {
    /// Positive finite target for the whole arrangement, summed across tracks.
    pub target_notes_per_bar: f64,
}

impl Evaluator<Composition> for DensityTarget {
    fn validate(&self) -> Result<(), EvaluationError> {
        if !self.target_notes_per_bar.is_finite() || self.target_notes_per_bar <= 0.0 {
            return Err(EvaluationError(
                "target_notes_per_bar must be positive and finite".into(),
            ));
        }
        Ok(())
    }

    fn evaluate(&self, score: &Composition) -> Result<Evaluation, EvaluationError> {
        self.validate()?;
        if score.meter.numerator == 0 || score.meter.denominator == 0 {
            return Err(EvaluationError("the score must have a valid meter".into()));
        }
        let ticks_per_bar = score.meter.ticks_per_bar().raw();
        if score.length.raw() <= 0 || ticks_per_bar <= 0 {
            return Err(EvaluationError(
                "the score must have a positive length in bars".into(),
            ));
        }
        let bars = score.length.raw() as f64 / ticks_per_bar as f64;
        let notes_per_bar = score.note_count() as f64 / bars;
        Ok(Evaluation {
            fitness: -(notes_per_bar - self.target_notes_per_bar).abs(),
            metrics: vec![
                Metric {
                    name: "notes_per_bar".into(),
                    value: notes_per_bar,
                },
                Metric {
                    name: "target_notes_per_bar".into(),
                    value: self.target_notes_per_bar,
                },
            ],
        })
    }
}

/// Complete inputs for one sequential composition search.
#[derive(Clone, Debug, PartialEq)]
pub struct SongSearchRequest {
    /// Existing specification; only selected dials and its seed may be replaced.
    pub base: SongSpec,
    /// Explicit bounded mutable dials.
    pub space: SearchSpace,
    /// Maximum proposed candidates, including failures; must be positive.
    pub attempt_budget: usize,
    /// Seed for proposals only, independent from musical randomness.
    pub search_seed: u64,
    /// Fixed composition seed, replacing `base.seed` in every candidate.
    pub composition_seed: u64,
    /// Proposal algorithm sharing the ordinary search runner.
    pub algorithm: SearchMethod,
    /// Evaluator configuration, fixed for the whole run.
    pub evaluator: DensityTarget,
}

impl SongSearchRequest {
    /// Rejects invalid configuration before proposing or composing any candidate.
    pub fn validate(&self) -> Result<(), SearchError> {
        if self.attempt_budget == 0 {
            return Err(SearchError::InvalidRequest(
                "attempt_budget must be greater than zero".into(),
            ));
        }
        validate_base(&self.base, &self.space)?;
        self.evaluator
            .validate()
            .map_err(SearchError::InvalidEvaluator)
    }
}

/// Adapter around the existing deterministic, synchronous Auris composer.
///
/// The explicit composition seed overrides `params.seed`. Repeatability is within the same
/// build; future writer improvements may change the generated music. The adapter has no audio,
/// filesystem or wall-clock inputs, and the existing composer has no mid-composition cancel hook.
#[derive(Clone, Copy, Debug, Default)]
pub struct AurisComposer;

impl Composer for AurisComposer {
    type Params = SongSpec;
    type Score = Composition;

    fn validate(&self, params: &SongSpec, _seed: u64) -> Result<(), ValidationError> {
        validate_spec(params)
    }

    fn compose(&self, params: &SongSpec, seed: u64) -> Result<Composition, ComposeError> {
        validate_spec(params).map_err(|error| ComposeError(error.to_string()))?;
        let mut params = params.clone();
        params.seed = seed;
        Ok(crate::compose(&params))
    }
}

pub(super) fn validate_base(base: &SongSpec, space: &SearchSpace) -> Result<(), SearchError> {
    validate_spec(base).map_err(|error| SearchError::InvalidRequest(error.to_string()))?;
    space.validate(base)
}

fn validate_spec(spec: &SongSpec) -> Result<(), ValidationError> {
    // Reuse the format's range and reference checks, then reject implicit normalization. The
    // latter catches in-memory shapes (missing sections, empty rosters) that the parser fills
    // from defaults; a retained candidate must mean exactly what its serialized inputs say.
    let reparsed = SongSpec::parse(&spec.to_toml()).map_err(|errors| {
        ValidationError(
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        )
    })?;
    if reparsed != *spec {
        return Err(ValidationError(
            "the specification changes when serialized; construct the base with SongSpec::parse before searching".into(),
        ));
    }
    Ok(())
}

/// Searches an existing song and retains the exact best generated score and ordered history.
///
/// Cancellation is checked between attempts. Each selected dial and all evaluator settings are
/// validated before the first proposal. The base's composition seed is replaced explicitly; every
/// other unselected field remains unchanged. Equal fitness keeps the earlier score.
pub fn search_composition(
    request: &SongSearchRequest,
    cancelled: impl FnMut() -> bool,
) -> Result<SearchResult<SongSpec, Composition>, SearchError> {
    request.validate()?;
    match request.algorithm {
        SearchMethod::Random => {
            let mut search = RandomSearch::new(
                request.base.clone(),
                request.space.clone(),
                request.search_seed,
                request.composition_seed,
            )?;
            run_search(
                &AurisComposer,
                &request.evaluator,
                &mut search,
                request.attempt_budget,
                cancelled,
            )
        }
        SearchMethod::HillClimb => {
            let mut search = HillClimber::new(
                request.base.clone(),
                request.space.clone(),
                request.search_seed,
                request.composition_seed,
            )?;
            run_search(
                &AurisComposer,
                &request.evaluator,
                &mut search,
                request.attempt_budget,
                cancelled,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use auris_core::{Note, Ticks};

    use super::*;
    use crate::search::{CandidateOutcome, TerminationReason};

    pub(super) fn base() -> SongSpec {
        SongSpec::parse(
            r#"
            title = "Search fixture"
            seed = 71
            form = ["verse"]
            ending = "none"
            [section.verse]
            bars = 4
            intensity = 0.5
            [[part]]
            name = "lead"
            role = "melody"
            density = 0.5
            "#,
        )
        .unwrap()
    }

    pub(super) fn space() -> SearchSpace {
        SearchSpace {
            part_density: Some(PartDensity {
                part: "lead".into(),
                bounds: ParameterBounds {
                    min: 0.0,
                    max: 1.0,
                    step: 0.25,
                },
            }),
            section_intensity: Some(SectionIntensity {
                section: "verse".into(),
                bounds: ParameterBounds {
                    min: 0.0,
                    max: 1.0,
                    step: 0.25,
                },
            }),
        }
    }

    fn request(algorithm: SearchMethod) -> SongSearchRequest {
        SongSearchRequest {
            base: base(),
            space: space(),
            attempt_budget: 12,
            search_seed: 3,
            composition_seed: 42,
            algorithm,
            evaluator: DensityTarget {
                target_notes_per_bar: 6.0,
            },
        }
    }

    #[test]
    fn real_search_is_repeatable_and_retains_the_best_exact_score_for_both_algorithms() {
        for method in [SearchMethod::Random, SearchMethod::HillClimb] {
            let request = request(method);
            let first = search_composition(&request, || false).unwrap();
            let second = search_composition(&request, || false).unwrap();
            assert_eq!(first.history, second.history);
            assert_eq!(first.best, second.best);
            assert_eq!(first.history.len(), request.attempt_budget);
            assert_eq!(first.termination, TerminationReason::BudgetExhausted);
            let best = first.best.as_ref().unwrap();
            assert_eq!(best.score, crate::compose(&best.candidate.params));
            assert_eq!(best.score.seed, request.composition_seed);
            assert_eq!(
                best.score.spec,
                best.candidate.params.to_toml(),
                "the exact generating configuration is retained"
            );
            let mut expected_best = None;
            for (index, attempt) in first.history.iter().enumerate() {
                assert_eq!(attempt.candidate.id, index);
                assert_eq!(attempt.candidate.seed, request.composition_seed);
                let CandidateOutcome::Success(evaluation) = &attempt.outcome else {
                    panic!("a real candidate failed: {:?}", attempt.outcome);
                };
                assert!(best.evaluation.fitness >= evaluation.fitness);
                if expected_best.is_none_or(|(_, fitness)| evaluation.fitness > fitness) {
                    expected_best = Some((index, evaluation.fitness));
                }
            }
            assert_eq!(best.candidate.id, expected_best.unwrap().0);
            assert!(
                first
                    .history
                    .windows(2)
                    .any(|pair| { pair[0].outcome != pair[1].outcome }),
                "the real objective must respond to the selected dials"
            );
        }
    }

    #[test]
    fn adapter_accepts_shipped_specs_and_overrides_only_the_seed() {
        AurisComposer.validate(&SongSpec::default(), 42).unwrap();
        for preset in crate::PRESETS {
            AurisComposer.validate(&preset.spec(), 42).unwrap();
        }
        let base = base();
        let score = AurisComposer.compose(&base, 42).unwrap();
        let mut expected = base.clone();
        expected.seed = 42;
        assert_eq!(score, crate::compose(&expected));
        assert_eq!(base.seed, 71);
    }

    #[test]
    fn the_full_composition_seed_range_remains_replayable() {
        let mut request = request(SearchMethod::HillClimb);
        request.base.seed = u64::MAX;
        request.composition_seed = u64::MAX;
        request.attempt_budget = 2;
        let result = search_composition(&request, || false).unwrap();
        assert_eq!(result.history.len(), 2);
        assert_eq!(result.best.as_ref().unwrap().score.seed, u64::MAX);
        let score = AurisComposer.compose(&base(), u64::MAX).unwrap();
        assert_eq!(SongSpec::parse(&score.spec).unwrap().seed, u64::MAX);
    }

    #[test]
    fn generated_inputs_stay_in_bounds_and_preserve_every_unselected_field() {
        for method in [SearchMethod::Random, SearchMethod::HillClimb] {
            let request = request(method);
            let result = search_composition(&request, || false).unwrap();
            for attempt in result.history {
                let mut params = attempt.candidate.params;
                request.space.validate(&params).unwrap();
                params.parts[0].density = request.base.parts[0].density;
                params.sections.get_mut("verse").unwrap().intensity =
                    request.base.sections["verse"].intensity;
                params.seed = request.base.seed;
                assert_eq!(params, request.base);
            }
        }
    }

    #[test]
    fn invalid_requests_are_rejected_before_cancellation_or_composition() {
        let mut invalid = Vec::new();
        let mut req = request(SearchMethod::Random);
        req.attempt_budget = 0;
        invalid.push(req);
        let mut req = request(SearchMethod::Random);
        req.base.tempo = f64::NAN;
        invalid.push(req);
        let mut req = request(SearchMethod::Random);
        req.base.parts.clear();
        invalid.push(req);
        let mut req = request(SearchMethod::Random);
        req.base.sections.clear();
        invalid.push(req);
        let mut req = request(SearchMethod::Random);
        req.base.parts[0].density = None;
        invalid.push(req);
        let mut req = request(SearchMethod::Random);
        req.space = SearchSpace::default();
        invalid.push(req);
        let mut req = request(SearchMethod::Random);
        req.space.part_density.as_mut().unwrap().part = "missing".into();
        invalid.push(req);
        let mut req = request(SearchMethod::Random);
        req.space.section_intensity.as_mut().unwrap().section = "missing".into();
        invalid.push(req);
        let mut req = request(SearchMethod::Random);
        req.space.part_density.as_mut().unwrap().bounds.min = 0.75;
        invalid.push(req);
        for target in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut req = request(SearchMethod::Random);
            req.evaluator.target_notes_per_bar = target;
            invalid.push(req);
        }
        for req in invalid {
            assert!(search_composition(&req, || panic!("invalid request started")).is_err());
        }
    }

    #[test]
    fn bounds_reject_invalid_ranges_and_steps() {
        for bounds in [
            ParameterBounds {
                min: f32::NAN,
                max: 1.0,
                step: 0.1,
            },
            ParameterBounds {
                min: 0.0,
                max: f32::INFINITY,
                step: 0.1,
            },
            ParameterBounds {
                min: -0.1,
                max: 1.0,
                step: 0.1,
            },
            ParameterBounds {
                min: 0.0,
                max: 1.1,
                step: 0.1,
            },
            ParameterBounds {
                min: 0.5,
                max: 0.5,
                step: 0.1,
            },
            ParameterBounds {
                min: 0.75,
                max: 0.25,
                step: 0.1,
            },
            ParameterBounds {
                min: 0.0,
                max: 1.0,
                step: 0.0,
            },
            ParameterBounds {
                min: 0.0,
                max: 1.0,
                step: f32::NAN,
            },
            ParameterBounds {
                min: 0.0,
                max: 1.0,
                step: 2.0,
            },
            ParameterBounds {
                min: 0.0,
                max: 1.0,
                step: f32::MIN_POSITIVE,
            },
        ] {
            assert!(bounds.validate().is_err(), "{bounds:?}");
        }
    }

    #[test]
    fn boundary_mutation_turns_inward_without_changing_more_than_the_step() {
        let bounds = ParameterBounds {
            min: 0.0,
            max: 1.0,
            step: 0.25,
        };
        assert_eq!(bounds.neighbor(0.0, false), 0.25);
        assert_eq!(bounds.neighbor(1.0, true), 0.75);
        assert_eq!(bounds.neighbor(0.125, false), 0.0);
        assert_eq!(bounds.neighbor(0.875, true), 1.0);
    }

    #[test]
    fn density_fitness_targets_the_requested_rate_on_curated_written_scores() {
        let mut score = crate::compose(&base());
        score.length = score.meter.ticks_per_bar() * 2;
        let template = score.tracks[0].clips[0].clone();
        score.tracks[0].clips = vec![template];
        let evaluator = DensityTarget {
            target_notes_per_bar: 2.0,
        };
        for (count, expected_rate, expected_fitness) in [
            (0, 0.0, -2.0),
            (2, 1.0, -1.0),
            (4, 2.0, 0.0),
            (6, 3.0, -1.0),
        ] {
            // Simultaneous chord tones still count separately; this is written-note density.
            score.tracks[0].clips[0].notes = (0..count)
                .map(|index| Note::new(60 + index, Ticks::ZERO, Ticks::QUARTER))
                .collect();
            let evaluation = evaluator.evaluate(&score).unwrap();
            assert_eq!(evaluation.fitness, expected_fitness);
            assert_eq!(evaluation.metrics[0].value, expected_rate);
            assert_eq!(evaluation.metrics[1].value, 2.0);
        }
        score.length = Ticks::ZERO;
        assert!(evaluator.evaluate(&score).is_err());
    }
}
