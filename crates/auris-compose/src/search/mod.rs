//! Sequential composition search with separate proposal, generation and evaluation steps.
//!
//! [`run_search`] keeps the exact highest-scoring output and reports every attempted input,
//! including failures. [`search_composition`] supplies the existing Auris composer, a bounded
//! song space and a fixed symbolic density target. No audio device or renderer is involved.

mod algorithms;
mod song;

pub use algorithms::{HillClimber, RandomSearch};
pub use song::{
    AurisComposer, DensityTarget, ParameterBounds, PartDensity, SearchMethod, SearchSpace,
    SectionIntensity, SongSearchRequest, search_composition,
};

/// One proposed configuration and the independent seed used to compose it.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate<P> {
    /// Zero-based proposal number, increasing even when an attempt fails.
    pub id: usize,
    /// The complete concrete composition parameters.
    pub params: P,
    /// Composition randomness; separate from the search algorithm's random stream.
    pub seed: u64,
}

/// A raw measurement explaining an evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct Metric {
    /// Stable name of the measured quantity.
    pub name: String,
    /// Finite measurement in the evaluator's documented units.
    pub value: f64,
}

/// The scalar to maximize and optional diagnostic measurements.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    /// Finite fitness; strictly larger values win.
    pub fitness: f64,
    /// Raw measurements, with fixed meaning and settings throughout one run.
    pub metrics: Vec<Metric>,
}

/// An invalid candidate configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError(pub String);

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValidationError {}

/// A failure to generate a score from a valid candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeError(pub String);

impl std::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ComposeError {}

/// A failed evaluation, including any nonfinite measurement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationError(pub String);

impl std::fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EvaluationError {}

/// Why a proposed candidate could not be ranked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateFailure {
    /// Rejected before composition.
    Validation(ValidationError),
    /// Generation failed after validation.
    Composition(ComposeError),
    /// Evaluation failed or produced NaN or infinity.
    Evaluation(EvaluationError),
}

/// Feedback delivered to the algorithm before it can propose the next candidate.
#[derive(Clone, Debug, PartialEq)]
pub enum CandidateOutcome {
    /// A successful, finite evaluation.
    Success(Evaluation),
    /// A candidate-level failure; this still consumed one attempt.
    Failure(CandidateFailure),
}

/// A generator independent of search state and evaluation settings.
pub trait Composer {
    /// Concrete inputs to generation.
    type Params;
    /// The generated artifact, retained without recomposition when it wins.
    type Score;

    /// Check an input before generating it; override for constrained domains.
    fn validate(&self, _params: &Self::Params, _seed: u64) -> Result<(), ValidationError> {
        Ok(())
    }

    /// Generate one artifact with the explicit composition seed.
    fn compose(&self, params: &Self::Params, seed: u64) -> Result<Self::Score, ComposeError>;
}

/// A fixed objective, independent of the search algorithm.
///
/// Keep configuration and any model snapshot constant for the whole run. The runner checks
/// both fitness and diagnostics for finiteness before forwarding feedback or comparing scores.
pub trait Evaluator<S> {
    /// Reject invalid evaluator configuration before any proposals are requested.
    fn validate(&self) -> Result<(), EvaluationError> {
        Ok(())
    }

    /// Measure one generated score, with larger fitness meaning better target agreement.
    fn evaluate(&self, score: &S) -> Result<Evaluation, EvaluationError>;
}

/// A proposal policy that sees inputs and feedback, never generated scores.
pub trait SearchAlgorithm<P> {
    /// Propose one candidate, or finish early. Assign sequential IDs starting at zero.
    fn ask(&mut self) -> Option<Candidate<P>>;

    /// Observe the previous proposal's outcome before the next call to [`Self::ask`].
    fn tell(&mut self, candidate: &Candidate<P>, outcome: &CandidateOutcome);
}

/// One history entry; unsuccessful generated scores are not kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Attempt<P> {
    /// Exact inputs of this attempt.
    pub candidate: Candidate<P>,
    /// Evaluation or a typed explanation of failure.
    pub outcome: CandidateOutcome,
}

/// The winning inputs, evaluation and exact generated score.
#[derive(Clone, Debug, PartialEq)]
pub struct BestCandidate<P, S> {
    /// Inputs that generated this score.
    pub candidate: Candidate<P>,
    /// The successful evaluation used to rank it.
    pub evaluation: Evaluation,
    /// The original generated artifact, never recomposed to construct the result.
    pub score: S,
}

/// Why the runner stopped requesting candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminationReason {
    /// Every allowed attempt was consumed, including failed candidates.
    BudgetExhausted,
    /// The search algorithm returned no further candidate.
    SearchExhausted,
    /// Cancellation was observed between attempts; partial results are retained.
    Cancelled,
}

/// Whether any attempted candidate could be evaluated successfully.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchStatus {
    /// The result includes a best candidate.
    BestFound,
    /// There is no valid candidate, including when cancelled before the first attempt.
    NoValidCandidate,
}

/// Candidate failures counted by the stage that rejected them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailureCounts {
    /// Invalid candidate inputs.
    pub validation: usize,
    /// Failed score generation.
    pub composition: usize,
    /// Failed or nonfinite evaluations.
    pub evaluation: usize,
}

/// Best artifact and ordered attempt history, including partial and all-failed runs.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult<P, S> {
    /// Exact best score, absent when no evaluation succeeded.
    pub best: Option<BestCandidate<P, S>>,
    /// Every proposal and outcome in the order they were attempted.
    pub history: Vec<Attempt<P>>,
    /// What stopped the loop, independent of whether it found a valid candidate.
    pub termination: TerminationReason,
    /// Failure counts available even when no candidate succeeded.
    pub failures: FailureCounts,
}

impl<P, S> SearchResult<P, S> {
    /// Distinguish a successful search from an explicit no-valid-candidate result.
    pub fn status(&self) -> SearchStatus {
        if self.best.is_some() {
            SearchStatus::BestFound
        } else {
            SearchStatus::NoValidCandidate
        }
    }
}

/// An unrecoverable configuration error, returned before the first proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchError {
    /// The attempt budget, base parameters or search bounds are invalid.
    InvalidRequest(String),
    /// Evaluator settings are invalid and cannot rank any candidate.
    InvalidEvaluator(EvaluationError),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid search request: {message}"),
            Self::InvalidEvaluator(error) => write!(f, "invalid search evaluator: {error}"),
        }
    }
}

impl std::error::Error for SearchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidEvaluator(error) => Some(error),
            Self::InvalidRequest(_) => None,
        }
    }
}

/// Run a sequential, bounded ask/compose/evaluate/tell loop.
///
/// Every proposal consumes an attempt. Failures are reported through `tell` and recorded;
/// equal fitness keeps the earlier candidate. The composer validates each proposal and the
/// evaluator validates its settings before the loop. Concrete search constructors validate
/// the base configuration and bounds. No score cloning or recomposition is required.
///
/// `cancelled` is checked before each attempt, so an in-flight composition/evaluation finishes
/// and remains in the partial result. The current Auris composer has no mid-call cancellation.
pub fn run_search<C, E, A>(
    composer: &C,
    evaluator: &E,
    search: &mut A,
    attempt_budget: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Result<SearchResult<C::Params, C::Score>, SearchError>
where
    C: Composer,
    C::Params: Clone,
    E: Evaluator<C::Score>,
    A: SearchAlgorithm<C::Params>,
{
    if attempt_budget == 0 {
        return Err(SearchError::InvalidRequest(
            "attempt budget must be positive".into(),
        ));
    }
    evaluator
        .validate()
        .map_err(SearchError::InvalidEvaluator)?;
    let mut result = SearchResult {
        best: None,
        history: Vec::new(),
        termination: TerminationReason::BudgetExhausted,
        failures: FailureCounts::default(),
    };
    for _ in 0..attempt_budget {
        if cancelled() {
            result.termination = TerminationReason::Cancelled;
            break;
        }
        let Some(candidate) = search.ask() else {
            result.termination = TerminationReason::SearchExhausted;
            break;
        };
        let attempted = composer
            .validate(&candidate.params, candidate.seed)
            .map_err(CandidateFailure::Validation)
            .and_then(|()| {
                composer
                    .compose(&candidate.params, candidate.seed)
                    .map_err(CandidateFailure::Composition)
            })
            .and_then(|score| {
                evaluator
                    .evaluate(&score)
                    .and_then(finite_evaluation)
                    .map(|evaluation| (score, evaluation))
                    .map_err(CandidateFailure::Evaluation)
            });
        let outcome =
            match attempted {
                Ok((score, evaluation)) => {
                    if result.best.as_ref().is_none_or(
                        |best: &BestCandidate<C::Params, C::Score>| {
                            evaluation.fitness > best.evaluation.fitness
                        },
                    ) {
                        result.best = Some(BestCandidate {
                            candidate: candidate.clone(),
                            evaluation: evaluation.clone(),
                            score,
                        });
                    }
                    CandidateOutcome::Success(evaluation)
                }
                Err(failure) => {
                    match &failure {
                        CandidateFailure::Validation(_) => result.failures.validation += 1,
                        CandidateFailure::Composition(_) => result.failures.composition += 1,
                        CandidateFailure::Evaluation(_) => result.failures.evaluation += 1,
                    }
                    CandidateOutcome::Failure(failure)
                }
            };
        search.tell(&candidate, &outcome);
        result.history.push(Attempt { candidate, outcome });
    }
    Ok(result)
}

fn finite_evaluation(evaluation: Evaluation) -> Result<Evaluation, EvaluationError> {
    if !evaluation.fitness.is_finite() {
        return Err(EvaluationError("fitness must be finite".into()));
    }
    if let Some(metric) = evaluation
        .metrics
        .iter()
        .find(|metric| !metric.value.is_finite())
    {
        return Err(EvaluationError(format!(
            "metric `{}` must be finite",
            metric.name
        )));
    }
    Ok(evaluation)
}

#[cfg(test)]
mod tests;
