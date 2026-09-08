//! Real modal gestures and worker ownership, without requiring an output device.

use auris_i18n::Language;
use auris_session::audio_evaluation::{AudioEvaluation, AudioEvaluator};
use auris_session::prelude::{Note, Ticks};
use gpui::{TestAppContext, px, size};

use super::*;
use crate::harness::{click, open, paint, resize, with_a_clip};

fn reference_audio() -> Arc<AudioBuffer> {
    let samples: Vec<_> = (0..44_100)
        .map(|index| ((index as f32 / 44_100.0) * 440.0 * std::f32::consts::TAU).sin() * 0.2)
        .collect();
    Arc::new(AudioBuffer::from_planar(vec![samples.clone(), samples], 44_100.0).unwrap())
}

fn configure(this: &mut AurisApp, cx: &mut Context<AurisApp>) {
    this.open_reference_match(cx);
    this.reference_match.settings.duration_seconds = 1.0;
    this.reference_match.settings.attempts = 4;
    this.reference_match.source = Some(ReferenceSource {
        path: "reference.wav".into(),
        audio: reference_audio(),
    });
    this.session.forget_history();
}

#[test]
fn excerpts_are_exact_and_short_or_invalid_requests_are_refused() {
    let audio = reference_audio();
    let excerpt = reference_excerpt(&audio, 0.25, 0.5).unwrap();
    assert_eq!(excerpt.frame_count(), 22_050);
    assert_eq!(excerpt.channel(0), &audio.channel(0)[11_025..33_075]);
    assert!(reference_excerpt(&audio, 0.5, 1.0).is_none());
    assert!(reference_excerpt(&audio, f64::NAN, 1.0).is_none());
}

#[gpui::test]
fn the_reference_modal_runs_on_pcm_without_editing_the_song(cx: &mut TestAppContext) {
    let (app, cx, _, clip) = with_a_clip(cx);
    let before = app.update(cx, |this, cx| {
        this.session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER * 2))
            .unwrap();
        configure(this, cx);
        this.project().clone()
    });
    paint(&app, cx);
    click("reference-match-start", cx);
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.reference_match.running.is_none());
        assert!(
            this.reference_match.error.is_none(),
            "{:?}",
            this.reference_match.error
        );
        let report = &this.reference_match.comparison.as_ref().unwrap().report;
        assert_eq!(report.attempts, 4);
        assert_eq!(report.baseline_audio.frame_count(), 44_100);
        assert!(report.best.fitness >= report.baseline.fitness);
        assert_eq!(this.project(), &before);
        assert!(!this.session.can_undo());
    });
    app.update(cx, |this, cx| {
        this.change_reference_settings(|state| state.settings.seed += 1);
        assert!(!this.apply_reference_match(cx));
        assert!(this.reference_match.error.is_some());
        assert_eq!(this.project(), &before);
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn cancellation_and_close_retain_the_worker_slot_until_it_returns(cx: &mut TestAppContext) {
    let (app, cx, _, clip) = with_a_clip(cx);
    app.update(cx, |this, cx| {
        this.session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        configure(this, cx);
        assert!(this.start_reference_match(cx));
        this.close_reference_match(cx);
        this.open_reference_match(cx);
        assert!(this.reference_match.running.is_some());
        assert!(!this.start_reference_match(cx));
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.reference_match.running.is_none());
        assert!(this.reference_match.comparison.is_none());
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn stop_invalidates_a_preview_that_is_still_being_prepared(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        configure(this, cx);
        this.preview_reference_match(0, cx);
        let requested = this.reference_match.preview_generation;
        this.stop_reference_preview();
        assert!(this.reference_match.preview_generation > requested);
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(!this.reference_match.previewing);
        assert!(!this.session.can_undo());
    });
}

struct FavorAmplitude;
impl AudioEvaluator for FavorAmplitude {
    fn evaluate(&self, audio: &AudioBuffer) -> Result<AudioEvaluation, String> {
        let energy = audio
            .iter_channels()
            .flatten()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>();
        Ok(AudioEvaluation {
            fitness: energy,
            metrics: Vec::new(),
        })
    }
    fn description(&self) -> String {
        "Energy measured from rendered PCM".into()
    }
}

#[gpui::test]
fn adopting_the_retained_project_is_one_undo(cx: &mut TestAppContext) {
    let (app, cx, _, clip) = with_a_clip(cx);
    app.update(cx, |this, cx| {
        this.session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER * 2))
            .unwrap();
        configure(this, cx);
        this.reference_match.settings.performance = false;
        this.reference_match.settings.attempts = 8;
        let before = this.project().clone();
        let settings = this.reference_match.settings.clone();
        let mut job = this
            .session
            .begin_reference_match(settings.clone(), Arc::new(FavorAmplitude))
            .unwrap();
        let report = loop {
            let measured = job.run(&AtomicBool::new(false), &mut |_| {}).unwrap();
            match this.session.continue_reference_match(measured).unwrap() {
                ReferenceMatchStep::Pending(next) => job = next,
                ReferenceMatchStep::Complete(report) => break report,
            }
        };
        assert!(report.best.fitness > report.baseline.fitness);
        let snapshot = MatchSnapshot {
            generation: this.reference_match.generation,
            revision: this.session.revision(),
            settings,
            reference_start: 0.0,
            reference: Arc::clone(&this.reference_match.source.as_ref().unwrap().audio),
        };
        this.reference_match.comparison = Some(MatchComparison { snapshot, report });
        assert_eq!(this.project(), &before);
        assert!(this.apply_reference_match(cx));
        let after = this.project().clone();
        assert_ne!(after, before);
        this.session.undo();
        assert_eq!(this.project(), &before);
        assert!(!this.session.can_undo());
        this.session.redo();
        assert_eq!(this.project(), &after);
    });
}

#[gpui::test]
fn invalid_reference_settings_and_missing_files_leave_the_project_unchanged(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    let before = app.update(cx, |this, cx| {
        this.open_reference_match(cx);
        assert!(!this.start_reference_match(cx));
        assert!(this.reference_match.error.is_some());
        this.load_reference_audio(PathBuf::from("missing-reference-file-for-test.wav"), cx);
        this.project().clone()
    });
    cx.run_until_parked();
    app.update(cx, |this, cx| {
        assert!(this.reference_match.error.is_some());
        assert!(this.reference_match.loading.is_none());
        configure(this, cx);
        this.reference_match.settings.duration_seconds = 2.0;
        assert!(this.start_reference_match(cx));
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.reference_match.error.is_some());
        assert!(this.reference_match.comparison.is_none());
        assert_eq!(this.project(), &before);
    });
}

#[gpui::test]
fn modal_actions_fit_both_languages_and_escape_closes(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| this.open_reference_match(cx));
    for language in [Language::English, Language::Japanese] {
        app.update(cx, |this, _| this.language = language);
        for width in [640.0, 900.0] {
            resize(&app, cx, size(px(width), px(650.0)));
            for selector in [
                "reference-match-panel",
                "reference-match-start",
                "reference-match-apply",
                "reference-match-close",
            ] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(bounds.left() >= px(0.0) && bounds.right() <= px(width));
                assert!(bounds.top() >= px(0.0) && bounds.bottom() <= px(650.0));
            }
        }
    }
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |this, _| assert!(!this.reference_match.open));
}
