use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureStage {
    Validation,
    Composition,
    Evaluation,
}

#[derive(Clone, Debug, PartialEq)]
struct Params {
    fitness: f64,
    diagnostic: f64,
    failure: Option<FailureStage>,
}

impl Params {
    fn successful(fitness: f64) -> Self {
        Self {
            fitness,
            diagnostic: 12.0,
            failure: None,
        }
    }

    fn failing(stage: FailureStage) -> Self {
        Self {
            failure: Some(stage),
            ..Self::successful(100.0)
        }
    }
}

// Intentionally not Clone: retaining a winner must work for an owned artifact.
#[derive(Debug, PartialEq)]
struct Score {
    params: Params,
    seed: u64,
    generation: usize,
    identity: Box<usize>,
}

#[derive(Default)]
struct Calls {
    candidate_validation: Cell<usize>,
    composition: Cell<usize>,
    evaluation_validation: Cell<usize>,
    evaluation: Cell<usize>,
    generated_addresses: RefCell<Vec<usize>>,
    cancelled: Cell<bool>,
}

struct MockComposer {
    calls: Rc<Calls>,
    cancel_during_composition: bool,
}

impl Composer for MockComposer {
    type Params = Params;
    type Score = Score;

    fn validate(&self, params: &Params, _seed: u64) -> Result<(), ValidationError> {
        self.calls
            .candidate_validation
            .set(self.calls.candidate_validation.get() + 1);
        if params.failure == Some(FailureStage::Validation) {
            Err(ValidationError("invalid candidate".into()))
        } else {
            Ok(())
        }
    }

    fn compose(&self, params: &Params, seed: u64) -> Result<Score, ComposeError> {
        let generation = self.calls.composition.get() + 1;
        self.calls.composition.set(generation);
        if self.cancel_during_composition {
            self.calls.cancelled.set(true);
        }
        if params.failure == Some(FailureStage::Composition) {
            return Err(ComposeError("composition failed".into()));
        }
        let identity = Box::new(generation);
        self.calls
            .generated_addresses
            .borrow_mut()
            .push(std::ptr::from_ref(identity.as_ref()) as usize);
        Ok(Score {
            params: params.clone(),
            seed,
            generation,
            identity,
        })
    }
}

struct MockEvaluator {
    calls: Rc<Calls>,
    invalid_configuration: bool,
    cancel_during_evaluation: bool,
}

impl Evaluator<Score> for MockEvaluator {
    fn validate(&self) -> Result<(), EvaluationError> {
        self.calls
            .evaluation_validation
            .set(self.calls.evaluation_validation.get() + 1);
        if self.invalid_configuration {
            Err(EvaluationError("invalid evaluator".into()))
        } else {
            Ok(())
        }
    }

    fn evaluate(&self, score: &Score) -> Result<Evaluation, EvaluationError> {
        self.calls.evaluation.set(self.calls.evaluation.get() + 1);
        if self.cancel_during_evaluation {
            self.calls.cancelled.set(true);
        }
        if score.params.failure == Some(FailureStage::Evaluation) {
            return Err(EvaluationError("evaluation failed".into()));
        }
        Ok(Evaluation {
            fitness: score.params.fitness,
            metrics: vec![
                Metric {
                    name: "fixed_reference".into(),
                    value: 1.0,
                },
                Metric {
                    name: "raw_measurement".into(),
                    value: score.params.diagnostic,
                },
            ],
        })
    }
}

struct ScriptedSearch {
    proposals: VecDeque<Candidate<Params>>,
    pending: Option<usize>,
    asks: usize,
    feedback: Vec<(usize, CandidateOutcome)>,
    feedback_count: Rc<Cell<usize>>,
}

impl ScriptedSearch {
    fn new(params: impl IntoIterator<Item = Params>) -> Self {
        Self {
            proposals: params
                .into_iter()
                .enumerate()
                .map(|(id, params)| Candidate {
                    id,
                    params,
                    seed: 73,
                })
                .collect(),
            pending: None,
            asks: 0,
            feedback: Vec::new(),
            feedback_count: Rc::default(),
        }
    }

    fn assert_feedback_matches(&self, history: &[Attempt<Params>]) {
        assert_eq!(self.pending, None, "the final proposal also needs feedback");
        assert_eq!(self.feedback.len(), history.len());
        for ((id, outcome), attempt) in self.feedback.iter().zip(history) {
            assert_eq!(*id, attempt.candidate.id);
            assert_eq!(outcome, &attempt.outcome);
        }
    }
}

impl SearchAlgorithm<Params> for ScriptedSearch {
    fn ask(&mut self) -> Option<Candidate<Params>> {
        assert_eq!(self.pending, None, "tell must precede the next ask");
        self.asks += 1;
        let candidate = self.proposals.pop_front()?;
        self.pending = Some(candidate.id);
        Some(candidate)
    }

    fn tell(&mut self, candidate: &Candidate<Params>, outcome: &CandidateOutcome) {
        assert_eq!(self.pending.take(), Some(candidate.id), "tell exactly once");
        self.feedback.push((candidate.id, outcome.clone()));
        self.feedback_count.set(self.feedback.len());
    }
}

fn mocks() -> (MockComposer, MockEvaluator, Rc<Calls>) {
    let calls = Rc::new(Calls::default());
    (
        MockComposer {
            calls: Rc::clone(&calls),
            cancel_during_composition: false,
        },
        MockEvaluator {
            calls: Rc::clone(&calls),
            invalid_configuration: false,
            cancel_during_evaluation: false,
        },
        calls,
    )
}

#[test]
fn every_proposal_consumes_budget_and_receives_typed_feedback_before_next_ask() {
    let (composer, evaluator, calls) = mocks();
    let mut search = ScriptedSearch::new([
        Params::successful(2.0),
        Params::failing(FailureStage::Validation),
        Params::failing(FailureStage::Composition),
        Params::failing(FailureStage::Evaluation),
        Params::successful(9.0),
    ]);
    let result = run_search(&composer, &evaluator, &mut search, 4, || false).unwrap();

    assert_eq!(result.termination, TerminationReason::BudgetExhausted);
    assert_eq!(result.status(), SearchStatus::BestFound);
    assert_eq!(result.best.as_ref().unwrap().candidate.id, 0);
    assert_eq!(result.history.len(), 4);
    assert_eq!(search.asks, 4);
    assert_eq!(calls.candidate_validation.get(), 4);
    assert_eq!(calls.composition.get(), 3);
    assert_eq!(calls.evaluation.get(), 2);
    assert_eq!(calls.evaluation_validation.get(), 1);
    assert_eq!(
        result.failures,
        FailureCounts {
            validation: 1,
            composition: 1,
            evaluation: 1
        }
    );
    assert_eq!(
        result.history[1].outcome,
        CandidateOutcome::Failure(CandidateFailure::Validation(ValidationError(
            "invalid candidate".into()
        )))
    );
    assert_eq!(
        result.history[2].outcome,
        CandidateOutcome::Failure(CandidateFailure::Composition(ComposeError(
            "composition failed".into()
        )))
    );
    assert_eq!(
        result.history[3].outcome,
        CandidateOutcome::Failure(CandidateFailure::Evaluation(EvaluationError(
            "evaluation failed".into()
        )))
    );
    search.assert_feedback_matches(&result.history);
}

#[test]
fn progress_observes_each_attempt_in_order_after_feedback_including_failures() {
    let (composer, evaluator, calls) = mocks();
    let mut search = ScriptedSearch::new([
        Params::successful(2.0),
        Params::failing(FailureStage::Validation),
        Params::failing(FailureStage::Composition),
        Params::failing(FailureStage::Evaluation),
        Params::successful(9.0),
    ]);
    let feedback_count = Rc::clone(&search.feedback_count);
    let mut observed = Vec::new();
    let result = run_search_with_progress(
        &composer,
        &evaluator,
        &mut search,
        5,
        || false,
        |attempt| {
            assert_eq!(attempt.candidate.id, observed.len());
            assert_eq!(feedback_count.get(), observed.len() + 1);
            observed.push(attempt.clone());
        },
    )
    .unwrap();

    assert_eq!(observed, result.history);
    assert_eq!(observed.len(), 5);
    assert_eq!(calls.composition.get(), 4);
    assert_eq!(calls.evaluation.get(), 3);
    search.assert_feedback_matches(&result.history);
}

#[test]
fn progress_can_cancel_before_the_next_attempt_and_keep_the_exact_best_artifact() {
    let (composer, evaluator, calls) = mocks();
    let mut search = ScriptedSearch::new([3.0, 1.0, 9.0].map(Params::successful));
    let mut observed = Vec::new();
    let result = run_search_with_progress(
        &composer,
        &evaluator,
        &mut search,
        5,
        || calls.cancelled.get(),
        |attempt| {
            observed.push(attempt.clone());
            if observed.len() == 2 {
                calls.cancelled.set(true);
            }
        },
    )
    .unwrap();

    assert_eq!(result.termination, TerminationReason::Cancelled);
    assert_eq!(observed, result.history);
    assert_eq!(search.asks, 2);
    assert_eq!(calls.composition.get(), 2);
    assert_eq!(calls.evaluation.get(), 2);
    search.assert_feedback_matches(&result.history);
    let best = result.best.unwrap();
    assert_eq!(best.candidate.id, 0);
    assert_eq!(best.score.generation, 1);
    assert_eq!(
        std::ptr::from_ref(best.score.identity.as_ref()) as usize,
        calls.generated_addresses.borrow()[0]
    );
}

#[test]
fn zero_budget_is_rejected_before_proposals_or_evaluator_validation() {
    let (composer, evaluator, calls) = mocks();
    let mut search = ScriptedSearch::new([Params::successful(1.0)]);
    let result = run_search(&composer, &evaluator, &mut search, 0, || false);
    assert!(matches!(result, Err(SearchError::InvalidRequest(_))));
    assert_eq!(search.asks, 0);
    assert_eq!(calls.evaluation_validation.get(), 0);
    assert_eq!(calls.candidate_validation.get(), 0);
    assert_eq!(calls.composition.get(), 0);
    assert_eq!(calls.evaluation.get(), 0);
}

#[test]
fn invalid_evaluator_configuration_is_rejected_before_any_proposal() {
    let (composer, mut evaluator, calls) = mocks();
    evaluator.invalid_configuration = true;
    let mut search = ScriptedSearch::new([Params::successful(1.0)]);
    let result = run_search(&composer, &evaluator, &mut search, 5, || false);
    assert_eq!(
        result.unwrap_err(),
        SearchError::InvalidEvaluator(EvaluationError("invalid evaluator".into()))
    );
    assert_eq!(search.asks, 0);
    assert_eq!(calls.evaluation_validation.get(), 1);
    assert_eq!(calls.candidate_validation.get(), 0);
    assert_eq!(calls.composition.get(), 0);
    assert_eq!(calls.evaluation.get(), 0);
}

#[test]
fn strict_ranking_retains_the_earlier_tied_artifact_without_recomposition() {
    let (composer, evaluator, calls) = mocks();
    let mut search = ScriptedSearch::new([1.0, 3.0, 3.0, 2.0].map(Params::successful));
    let result = run_search(&composer, &evaluator, &mut search, 4, || false).unwrap();
    search.assert_feedback_matches(&result.history);
    let best = result.best.unwrap();
    assert_eq!(best.candidate.id, 1);
    assert_eq!(best.evaluation.fitness, 3.0);
    assert_eq!(best.score.generation, 2);
    assert_eq!(best.score.seed, best.candidate.seed);
    assert_eq!(best.score.params, best.candidate.params);
    assert_eq!(
        std::ptr::from_ref(best.score.identity.as_ref()) as usize,
        calls.generated_addresses.borrow()[1]
    );
    assert_eq!(*best.score.identity, 2);
    assert_eq!(calls.composition.get(), 4);
    assert_eq!(calls.evaluation.get(), 4);
}

#[test]
fn nonfinite_fitness_is_a_typed_failure_before_feedback() {
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let (composer, evaluator, calls) = mocks();
        let mut search =
            ScriptedSearch::new([Params::successful(invalid), Params::successful(0.0)]);
        let result = run_search(&composer, &evaluator, &mut search, 2, || false).unwrap();
        assert_eq!(
            result.history[0].outcome,
            CandidateOutcome::Failure(CandidateFailure::Evaluation(EvaluationError(
                "fitness must be finite".into()
            )))
        );
        assert_eq!(result.failures.evaluation, 1);
        assert_eq!(result.best.as_ref().unwrap().candidate.id, 1);
        assert_eq!(calls.evaluation.get(), 2);
        search.assert_feedback_matches(&result.history);
    }
}

#[test]
fn nonfinite_raw_diagnostics_cannot_win_even_with_finite_high_fitness() {
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let (composer, evaluator, _) = mocks();
        let mut params = Params::successful(100.0);
        params.diagnostic = invalid;
        let mut search = ScriptedSearch::new([Params::successful(-5.0), params]);
        let result = run_search(&composer, &evaluator, &mut search, 2, || false).unwrap();
        assert_eq!(
            result.history[1].outcome,
            CandidateOutcome::Failure(CandidateFailure::Evaluation(EvaluationError(
                "metric `raw_measurement` must be finite".into()
            )))
        );
        assert_eq!(result.failures.evaluation, 1);
        assert_eq!(result.best.as_ref().unwrap().candidate.id, 0);
        search.assert_feedback_matches(&result.history);
    }
}

#[test]
fn search_exhaustion_returns_partial_or_empty_results_without_spending_extra_attempts() {
    for proposal_count in [0, 2] {
        let (composer, evaluator, calls) = mocks();
        let mut search =
            ScriptedSearch::new((0..proposal_count).map(|id| Params::successful(f64::from(id))));
        let result = run_search(&composer, &evaluator, &mut search, 5, || false).unwrap();
        assert_eq!(result.termination, TerminationReason::SearchExhausted);
        assert_eq!(result.history.len(), proposal_count as usize);
        assert_eq!(search.asks, proposal_count as usize + 1);
        assert_eq!(calls.composition.get(), proposal_count as usize);
        assert_eq!(result.best.is_some(), proposal_count > 0);
        assert_eq!(result.failures, FailureCounts::default());
        search.assert_feedback_matches(&result.history);
    }
}

#[test]
fn cancellation_before_first_attempt_returns_explicit_no_valid_candidate() {
    let (composer, evaluator, calls) = mocks();
    let mut search = ScriptedSearch::new([Params::successful(1.0)]);
    let result = run_search(&composer, &evaluator, &mut search, 5, || true).unwrap();
    assert_eq!(result.termination, TerminationReason::Cancelled);
    assert_eq!(result.status(), SearchStatus::NoValidCandidate);
    assert!(result.history.is_empty());
    assert!(result.best.is_none());
    assert_eq!(result.failures, FailureCounts::default());
    assert_eq!(search.asks, 0);
    assert_eq!(calls.composition.get(), 0);
    search.assert_feedback_matches(&result.history);
}

#[test]
fn cancellation_between_attempts_preserves_history_and_the_best_artifact() {
    let (composer, evaluator, _) = mocks();
    let mut search = ScriptedSearch::new([2.0, 1.0, 9.0].map(Params::successful));
    let mut cancellation_checks = 0;
    let result = run_search(&composer, &evaluator, &mut search, 5, || {
        cancellation_checks += 1;
        cancellation_checks > 2
    })
    .unwrap();
    assert_eq!(result.termination, TerminationReason::Cancelled);
    assert_eq!(result.status(), SearchStatus::BestFound);
    assert_eq!(result.history.len(), 2);
    assert_eq!(search.asks, 2);
    assert_eq!(result.best.as_ref().unwrap().candidate.id, 0);
    assert_eq!(result.best.as_ref().unwrap().score.generation, 1);
    search.assert_feedback_matches(&result.history);
}

#[test]
fn cancellation_during_composition_or_evaluation_keeps_the_completed_attempt() {
    for cancel_in_composer in [true, false] {
        let (mut composer, mut evaluator, calls) = mocks();
        composer.cancel_during_composition = cancel_in_composer;
        evaluator.cancel_during_evaluation = !cancel_in_composer;
        let mut search = ScriptedSearch::new([1.0, 9.0].map(Params::successful));
        let result = run_search(&composer, &evaluator, &mut search, 5, || {
            calls.cancelled.get()
        })
        .unwrap();
        assert_eq!(result.termination, TerminationReason::Cancelled);
        assert_eq!(result.status(), SearchStatus::BestFound);
        assert_eq!(result.history.len(), 1);
        assert_eq!(search.asks, 1);
        assert_eq!(result.best.as_ref().unwrap().candidate.id, 0);
        assert_eq!(calls.composition.get(), 1);
        assert_eq!(calls.evaluation.get(), 1);
        search.assert_feedback_matches(&result.history);
    }
}

#[test]
fn all_failed_attempts_return_failure_counts_and_no_valid_candidate() {
    let (composer, evaluator, _) = mocks();
    let mut search = ScriptedSearch::new([
        Params::failing(FailureStage::Validation),
        Params::failing(FailureStage::Composition),
        Params::failing(FailureStage::Evaluation),
        Params::successful(f64::NAN),
    ]);
    let result = run_search(&composer, &evaluator, &mut search, 4, || false).unwrap();
    assert_eq!(result.status(), SearchStatus::NoValidCandidate);
    assert_eq!(result.termination, TerminationReason::BudgetExhausted);
    assert!(result.best.is_none());
    assert_eq!(result.history.len(), 4);
    assert_eq!(
        result.failures,
        FailureCounts {
            validation: 1,
            composition: 1,
            evaluation: 2
        }
    );
    search.assert_feedback_matches(&result.history);
}

#[test]
fn deterministic_runs_retain_identical_results_and_nondecreasing_best_across_budgets() {
    let params = [
        Params::successful(-5.0),
        Params::successful(-8.0),
        Params::failing(FailureStage::Validation),
        Params::successful(-2.0),
        Params::failing(FailureStage::Composition),
        Params::successful(-2.0),
        Params::failing(FailureStage::Evaluation),
        Params::successful(4.0),
    ];
    let mut previous_best = f64::NEG_INFINITY;
    for budget in 1..=params.len() {
        let run = || {
            let (composer, evaluator, _) = mocks();
            let mut search = ScriptedSearch::new(params.clone());
            let result = run_search(&composer, &evaluator, &mut search, budget, || false).unwrap();
            search.assert_feedback_matches(&result.history);
            result
        };
        let result = run();
        assert_eq!(result, run());
        let fitness = result.best.as_ref().unwrap().evaluation.fitness;
        assert!(fitness >= previous_best);
        previous_best = fitness;
        assert_eq!(result.history.len(), budget);
        assert_eq!(result.termination, TerminationReason::BudgetExhausted);
    }
}
