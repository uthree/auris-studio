//! Invalid candidates consume budget without displacing a measured winner.

use std::sync::atomic::AtomicUsize;

use auris_core::{Note, Ticks};

use super::*;
use crate::SessionOptions;
use crate::audio_evaluation::AudioMetric;

#[derive(Clone, Copy)]
enum Outcome {
    Score(f64),
    Error,
    InvalidMetric,
}

struct ScriptedEvaluator {
    outcomes: Vec<Outcome>,
    calls: AtomicUsize,
}

impl ScriptedEvaluator {
    fn new(outcomes: Vec<Outcome>) -> Arc<Self> {
        Arc::new(Self {
            outcomes,
            calls: AtomicUsize::new(0),
        })
    }
}

impl AudioEvaluator for ScriptedEvaluator {
    fn evaluate(&self, audio: &AudioBuffer) -> Result<AudioEvaluation, String> {
        assert!(audio.frame_count() > 0);
        match self.outcomes[self.calls.fetch_add(1, Ordering::Relaxed)] {
            Outcome::Score(fitness) => Ok(AudioEvaluation {
                fitness,
                metrics: Vec::new(),
            }),
            Outcome::Error => Err("candidate has no measurable sound".into()),
            Outcome::InvalidMetric => Ok(AudioEvaluation {
                fitness: 999.0,
                metrics: vec![AudioMetric {
                    name: "invalid diagnostic".into(),
                    value: f64::NAN,
                }],
            }),
        }
    }

    fn description(&self) -> String {
        "Scripted outcomes over actual rendered candidates".into()
    }
}

fn fixture() -> Session {
    let mut session = Session::new(SessionOptions::headless()).unwrap();
    let track = session.add_default_instrument_track("Lead").unwrap();
    let clip = session
        .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::from_beats(2.0))
        .unwrap();
    session
        .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
        .unwrap();
    session.forget_history();
    session
}

fn settings(attempts: usize) -> ReferenceMatchSettings {
    ReferenceMatchSettings {
        duration_seconds: 1.0,
        attempts,
        performance: false,
        generation_seeds: false,
        instruments: false,
        arrangement: false,
        ..ReferenceMatchSettings::default()
    }
}

#[test]
fn a_baseline_evaluation_error_or_invalid_score_remains_fatal() {
    for outcome in [
        Outcome::Error,
        Outcome::Score(f64::NAN),
        Outcome::InvalidMetric,
    ] {
        let mut session = fixture();
        let before = session.project.clone();
        let evaluator = ScriptedEvaluator::new(vec![outcome]);
        let job = session
            .begin_reference_match(settings(4), evaluator.clone())
            .unwrap();
        assert!(job.run(&AtomicBool::new(false), &mut |_| {}).is_err());
        assert_eq!(evaluator.calls.load(Ordering::Relaxed), 1);
        assert_eq!(session.project, before);
        assert!(!session.can_undo());
    }
}

#[test]
fn failed_middle_candidates_keep_the_exact_winner_and_consume_the_budget() {
    let mut session = fixture();
    let before = session.project.clone();
    let evaluator = ScriptedEvaluator::new(vec![
        Outcome::Score(0.0),
        Outcome::Error,
        Outcome::Score(3.0),
        Outcome::Score(f64::NAN),
        Outcome::Score(2.0),
        Outcome::InvalidMetric,
    ]);
    let mut job = session
        .begin_reference_match(settings(6), evaluator.clone())
        .unwrap();
    let mut expected = None;
    let mut fractions = Vec::new();
    let report = loop {
        if job.progress().completed == 2 {
            expected = Some(job.state.candidate.clone());
        }
        let result = job
            .run(&AtomicBool::new(false), &mut |fraction| {
                fractions.push(fraction)
            })
            .unwrap();
        assert_eq!(session.project, before);
        assert!(!session.can_undo());
        match session.continue_reference_match(result).unwrap() {
            ReferenceMatchStep::Pending(next) => job = next,
            ReferenceMatchStep::Complete(report) => break report,
        }
    };
    assert_eq!(evaluator.calls.load(Ordering::Relaxed), 6);
    assert_eq!(report.attempts, 6);
    assert_eq!(report.failed_attempts, 3);
    assert_eq!(report.best.fitness, 3.0);
    assert!(!report.cancelled);
    assert_eq!(fractions.last(), Some(&1.0));
    assert!(fractions.windows(2).all(|pair| pair[0] <= pair[1]));
    let expected = expected.unwrap();
    assert_ne!(expected, before);
    assert_eq!(report.adoption.project, expected);
    assert!(session.apply_reference_match(&report).unwrap());
    assert_eq!(session.project, expected);
    session.undo();
    assert_eq!(session.project, before);
    assert!(!session.can_undo());
}

#[test]
fn when_all_later_candidates_fail_the_baseline_remains_available() {
    let mut session = fixture();
    let before = session.project.clone();
    let evaluator = ScriptedEvaluator::new(vec![
        Outcome::Score(1.0),
        Outcome::Error,
        Outcome::InvalidMetric,
        Outcome::Score(f64::INFINITY),
    ]);
    let mut job = session
        .begin_reference_match(settings(4), evaluator)
        .unwrap();
    let report = loop {
        let result = job.run(&AtomicBool::new(false), &mut |_| {}).unwrap();
        match session.continue_reference_match(result).unwrap() {
            ReferenceMatchStep::Pending(next) => job = next,
            ReferenceMatchStep::Complete(report) => break report,
        }
    };
    assert_eq!(report.attempts, 4);
    assert_eq!(report.failed_attempts, 3);
    assert!(!report.cancelled);
    assert!(Arc::ptr_eq(&report.baseline_audio, &report.best_audio));
    assert_eq!(report.baseline, report.best);
    assert!(report.changes.is_empty());
    assert!(!session.apply_reference_match(&report).unwrap());
    assert_eq!(session.project, before);
    assert!(!session.can_undo());
}

#[test]
fn a_render_failure_after_the_baseline_consumes_one_attempt() {
    let mut session = fixture();
    let evaluator = ScriptedEvaluator::new(vec![Outcome::Score(0.0), Outcome::Score(1.0)]);
    let mut invalid_baseline = session
        .begin_reference_match(settings(3), evaluator.clone())
        .unwrap();
    invalid_baseline.state.options.start_frames = 1;
    invalid_baseline.state.options.end_frames = Some(0);
    assert!(
        invalid_baseline
            .run(&AtomicBool::new(false), &mut |_| {})
            .is_err()
    );
    assert_eq!(evaluator.calls.load(Ordering::Relaxed), 0);
    let job = session
        .begin_reference_match(settings(3), evaluator.clone())
        .unwrap();
    let result = job.run(&AtomicBool::new(false), &mut |_| {}).unwrap();
    let ReferenceMatchStep::Pending(mut job) = session.continue_reference_match(result).unwrap()
    else {
        panic!("the next render is pending")
    };
    // An invalid range fails the renderer before it reaches the evaluator.
    job.state.options.start_frames = 1;
    job.state.options.end_frames = Some(0);
    let mut result = job.run(&AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(result.state.failed_attempts, 1);
    assert_eq!(result.state.completed, 2);
    assert_eq!(evaluator.calls.load(Ordering::Relaxed), 1);
    result.state.options.start_frames = 0;
    result.state.options.end_frames = Some(SAMPLE_RATE as u64);
    let ReferenceMatchStep::Pending(job) = session.continue_reference_match(result).unwrap() else {
        panic!("a failed candidate leaves budget for the next render")
    };
    let result = job.run(&AtomicBool::new(false), &mut |_| {}).unwrap();
    let ReferenceMatchStep::Complete(report) = session.continue_reference_match(result).unwrap()
    else {
        panic!("all three attempts finished")
    };
    assert_eq!(report.attempts, 3);
    assert_eq!(report.failed_attempts, 1);
    assert_eq!(report.best.fitness, 1.0);
}

struct CancelWithError {
    cancel: Arc<AtomicBool>,
    calls: AtomicUsize,
}

impl AudioEvaluator for CancelWithError {
    fn evaluate(&self, _: &AudioBuffer) -> Result<AudioEvaluation, String> {
        if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
            Ok(AudioEvaluation {
                fitness: 1.0,
                metrics: Vec::new(),
            })
        } else {
            self.cancel.store(true, Ordering::Relaxed);
            Err("evaluation interrupted".into())
        }
    }

    fn description(&self) -> String {
        "Cancellation arriving during a failed evaluation".into()
    }
}

#[test]
fn cancellation_during_a_failed_evaluation_is_not_counted_as_a_rejected_candidate() {
    let mut session = fixture();
    let cancel = Arc::new(AtomicBool::new(false));
    let evaluator = Arc::new(CancelWithError {
        cancel: Arc::clone(&cancel),
        calls: AtomicUsize::new(0),
    });
    let job = session
        .begin_reference_match(settings(4), evaluator)
        .unwrap();
    let result = job.run(&cancel, &mut |_| {}).unwrap();
    let ReferenceMatchStep::Pending(job) = session.continue_reference_match(result).unwrap() else {
        panic!("candidate after baseline")
    };
    let result = job.run(&cancel, &mut |_| {}).unwrap();
    let ReferenceMatchStep::Complete(report) = session.continue_reference_match(result).unwrap()
    else {
        panic!("cancellation returns the partial report")
    };
    assert!(report.cancelled);
    assert_eq!(report.attempts, 1);
    assert_eq!(report.failed_attempts, 0);
    assert!(Arc::ptr_eq(&report.baseline_audio, &report.best_audio));
    assert!(!session.can_undo());
}
