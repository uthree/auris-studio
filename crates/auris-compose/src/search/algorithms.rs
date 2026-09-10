//! Independent random sampling and feedback-driven single-parameter hill climbing.

use crate::SongSpec;
use crate::rng::Rng;

use super::song::{SearchSpace, validate_base};
use super::{Candidate, CandidateOutcome, SearchAlgorithm, SearchError};

/// Independent uniform proposals in the selected density and intensity bounds.
///
/// Outcomes are reported through the common protocol but do not alter future proposals. Every
/// proposal has the same composition seed; only a separate search RNG samples the dials.
#[derive(Clone, Debug)]
pub struct RandomSearch {
    base: SongSpec,
    space: SearchSpace,
    rng: Rng,
    next_id: usize,
}

impl RandomSearch {
    /// Validates the base and space, then constructs an independent seeded sampler.
    pub fn new(
        mut base: SongSpec,
        space: SearchSpace,
        search_seed: u64,
        composition_seed: u64,
    ) -> Result<Self, SearchError> {
        validate_base(&base, &space)?;
        base.seed = composition_seed;
        Ok(Self {
            base,
            space,
            rng: Rng::stream(
                search_seed,
                &["composition-search", "random"].map(Into::into),
            ),
            next_id: 0,
        })
    }
}

impl SearchAlgorithm<SongSpec> for RandomSearch {
    fn ask(&mut self) -> Option<Candidate<SongSpec>> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        Some(Candidate {
            id,
            params: self.space.sample(&self.base, &mut self.rng),
            seed: self.base.seed,
        })
    }

    fn tell(&mut self, _candidate: &Candidate<SongSpec>, _outcome: &CandidateOutcome) {}
}

/// A single incumbent updated only by strictly better successful feedback.
///
/// The base is proposed first. After its first success, each proposal changes exactly one
/// selected field of the incumbent by the configured step, clamped to the bounds. An outward
/// move at a boundary turns inward. Until a proposal succeeds, new parameters are sampled
/// independently. Ties and failures keep the current parent. This can settle at a local optimum.
#[derive(Clone, Debug)]
pub struct HillClimber {
    base: SongSpec,
    space: SearchSpace,
    rng: Rng,
    next_id: usize,
    incumbent: Option<(SongSpec, f64)>,
}

impl HillClimber {
    /// Validates the base and space, then constructs a feedback-driven seeded hill climber.
    pub fn new(
        mut base: SongSpec,
        space: SearchSpace,
        search_seed: u64,
        composition_seed: u64,
    ) -> Result<Self, SearchError> {
        validate_base(&base, &space)?;
        base.seed = composition_seed;
        Ok(Self {
            base,
            space,
            rng: Rng::stream(search_seed, &["composition-search", "hill"].map(Into::into)),
            next_id: 0,
            incumbent: None,
        })
    }
}

impl SearchAlgorithm<SongSpec> for HillClimber {
    fn ask(&mut self) -> Option<Candidate<SongSpec>> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        let params = if id == 0 {
            self.base.clone()
        } else if let Some((parent, _)) = &self.incumbent {
            self.space.neighbor(parent, &mut self.rng)
        } else {
            self.space.sample(&self.base, &mut self.rng)
        };
        Some(Candidate {
            id,
            params,
            seed: self.base.seed,
        })
    }

    fn tell(&mut self, candidate: &Candidate<SongSpec>, outcome: &CandidateOutcome) {
        if let CandidateOutcome::Success(evaluation) = outcome
            && evaluation.fitness.is_finite()
            && self
                .incumbent
                .as_ref()
                .is_none_or(|(_, fitness)| evaluation.fitness > *fitness)
        {
            self.incumbent = Some((candidate.params.clone(), evaluation.fitness));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{
        CandidateFailure, ComposeError, Composer, Evaluation, EvaluationError, Evaluator,
        ParameterBounds, PartDensity, ValidationError, run_search,
    };

    fn base() -> SongSpec {
        SongSpec::parse(
            r#"
            form = ["verse"]
            [section.verse]
            bars = 2
            [[part]]
            name = "lead"
            density = 0.0
            "#,
        )
        .unwrap()
    }

    fn space() -> SearchSpace {
        SearchSpace {
            part_density: Some(PartDensity {
                part: "lead".into(),
                bounds: ParameterBounds {
                    min: 0.0,
                    max: 1.0,
                    step: 0.25,
                },
            }),
            section_intensity: None,
        }
    }

    fn success(fitness: f64) -> CandidateOutcome {
        CandidateOutcome::Success(Evaluation {
            fitness,
            metrics: Vec::new(),
        })
    }

    fn hill() -> HillClimber {
        HillClimber::new(base(), space(), 3, 42).unwrap()
    }

    fn density(candidate: &Candidate<SongSpec>) -> f32 {
        candidate.params.parts[0].density.unwrap()
    }

    #[test]
    fn feedback_changes_the_parent_of_the_next_controlled_step() {
        let mut improved = hill();
        let mut tied = hill();
        let mut failed = hill();
        for search in [&mut improved, &mut tied, &mut failed] {
            let first = search.ask().unwrap();
            let mut expected = base();
            expected.seed = 42;
            assert_eq!(first.params, expected);
            assert_eq!(first.id, 0);
            search.tell(&first, &success(0.0));
        }
        let better = improved.ask().unwrap();
        let same = tied.ask().unwrap();
        let bad = failed.ask().unwrap();
        // The base is at the lower bound; either chosen direction makes this exact step.
        assert_eq!(density(&better), 0.25);
        assert_eq!(better, same);
        assert_eq!(better, bad);
        improved.tell(&better, &success(1.0));
        tied.tell(&same, &success(0.0));
        failed.tell(
            &bad,
            &CandidateOutcome::Failure(CandidateFailure::Validation(ValidationError(
                "fixture".into(),
            ))),
        );
        let after_improvement = improved.ask().unwrap();
        let after_tie = tied.ask().unwrap();
        let after_failure = failed.ask().unwrap();
        assert_eq!(density(&after_tie), 0.25);
        assert_eq!(after_tie, after_failure);
        assert!(matches!(density(&after_improvement), 0.0 | 0.5));
        assert_ne!(after_improvement.params, after_tie.params);
        assert_eq!(after_improvement.id, 2);
        assert_eq!(after_improvement.seed, 42);
    }

    #[test]
    fn failed_base_samples_again_until_the_first_success() {
        let mut search = hill();
        let first = search.ask().unwrap();
        search.tell(
            &first,
            &CandidateOutcome::Failure(CandidateFailure::Composition(ComposeError(
                "fixture".into(),
            ))),
        );
        let sampled = search.ask().unwrap();
        assert_eq!(sampled.id, 1);
        assert!(search.incumbent.is_none());
        space().validate(&sampled.params).unwrap();
        search.tell(&sampled, &success(-100.0));
        assert_eq!(search.incumbent.as_ref().unwrap().0, sampled.params);
    }

    #[test]
    fn random_proposals_ignore_feedback_and_use_a_separate_seed() {
        let mut left = RandomSearch::new(base(), space(), 3, 42).unwrap();
        let mut right = left.clone();
        let mut other_composition_seed = RandomSearch::new(base(), space(), 3, 99).unwrap();
        for index in 0..20 {
            let a = left.ask().unwrap();
            let b = right.ask().unwrap();
            let c = other_composition_seed.ask().unwrap();
            assert_eq!(a, b);
            assert_eq!(density(&a), density(&c));
            assert_eq!(a.id, index);
            assert_eq!(c.seed, 99);
            left.tell(&a, &success(index as f64));
            right.tell(
                &b,
                &CandidateOutcome::Failure(CandidateFailure::Evaluation(EvaluationError(
                    "fixture".into(),
                ))),
            );
        }
    }

    struct DensityComposer;

    impl Composer for DensityComposer {
        type Params = SongSpec;
        type Score = f64;

        fn compose(&self, params: &SongSpec, _seed: u64) -> Result<f64, ComposeError> {
            Ok(f64::from(params.parts[0].density.unwrap()))
        }
    }

    struct IncreasingDensity;

    impl Evaluator<f64> for IncreasingDensity {
        fn evaluate(&self, score: &f64) -> Result<Evaluation, EvaluationError> {
            Ok(Evaluation {
                fitness: *score,
                metrics: Vec::new(),
            })
        }
    }

    #[test]
    fn controlled_bounded_objective_never_lowers_the_incumbent() {
        let mut search = hill();
        let result = run_search(
            &DensityComposer,
            &IncreasingDensity,
            &mut search,
            30,
            || false,
        )
        .unwrap();
        let mut parent = 0.0;
        for attempt in &result.history {
            let value = density(&attempt.candidate);
            assert!((value - parent).abs() <= 0.25);
            let CandidateOutcome::Success(evaluation) = &attempt.outcome else {
                panic!("synthetic objective failed");
            };
            parent = parent.max(evaluation.fitness as f32);
        }
        assert_eq!(parent, 1.0);
        assert_eq!(result.best.unwrap().evaluation.fitness, 1.0);
        assert_eq!(search.incumbent.unwrap().1, 1.0);
    }

    #[test]
    fn hill_proposals_mutate_exactly_one_selected_parameter() {
        let mut base = base();
        base.sections.get_mut("verse").unwrap().intensity = 0.5;
        let mut space = space();
        space.section_intensity = Some(crate::search::SectionIntensity {
            section: "verse".into(),
            bounds: ParameterBounds {
                min: 0.0,
                max: 1.0,
                step: 0.25,
            },
        });
        let mut search = HillClimber::new(base, space.clone(), 3, 42).unwrap();
        let mut parent = search.ask().unwrap();
        search.tell(&parent, &success(0.0));
        let mut changed_density = false;
        let mut changed_intensity = false;
        for index in 1..40 {
            let candidate = search.ask().unwrap();
            let density_changed = density(&candidate) != density(&parent);
            let intensity_changed = candidate.params.sections["verse"].intensity
                != parent.params.sections["verse"].intensity;
            assert_ne!(density_changed, intensity_changed);
            changed_density |= density_changed;
            changed_intensity |= intensity_changed;
            space.validate(&candidate.params).unwrap();
            search.tell(&candidate, &success(index as f64));
            parent = candidate;
        }
        assert!(changed_density && changed_intensity);
    }
}
