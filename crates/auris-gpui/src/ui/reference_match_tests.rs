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
        assert_eq!(this.reference_match.preview.as_ref().unwrap().selection, 0);
        assert!(
            this.reference_match
                .preview
                .as_ref()
                .unwrap()
                .status
                .is_none()
        );
        let requested = this.reference_match.preview_generation;
        this.stop_reference_preview();
        assert!(this.reference_match.preview_generation > requested);
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.reference_match.preview.is_none());
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
            reference: Some(Arc::clone(
                &this.reference_match.source.as_ref().unwrap().audio,
            )),
            objective: this.reference_match.objective,
            model_directory: this.reference_match.model_directory.clone(),
            text_prompt: this.reference_match.text_prompt.clone(),
        };
        this.reference_match.comparison = Some(MatchComparison { snapshot, report });
        assert_eq!(this.project(), &before);
        assert!(this.apply_reference_match(cx));
        assert!(!this.reference_match.open);
        assert!(this.reference_match.comparison.is_none());
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
        for (width, objective) in [
            (640.0, MatchObjective::AcousticReference),
            (900.0, MatchObjective::AcousticReference),
            (640.0, MatchObjective::ClapReference),
            (900.0, MatchObjective::ClapText),
        ] {
            app.update(cx, |this, _| this.reference_match.objective = objective);
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

#[gpui::test]
fn clap_text_form_accepts_a_prompt_without_reference_audio(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let before = app.update(cx, |this, cx| {
        this.open_reference_match(cx);
        this.project().clone()
    });
    paint(&app, cx);
    click("audio-match-clap-text", cx);
    app.update(cx, |this, cx| {
        assert_eq!(this.reference_match.objective, MatchObjective::ClapText);
        assert_eq!(
            this.reference_match.input_problem(),
            Some(Key::AudioMatchModelRequired)
        );
        assert!(!this.start_reference_match(cx));
        this.change_reference_settings(|state| {
            state.model_directory = Some("missing-clap-model-for-test".into())
        });
        assert_eq!(
            this.reference_match.input_problem(),
            Some(Key::AudioMatchPromptRequired)
        );
    });
    paint(&app, cx);
    click("audio-match-prompt", cx);
    paint(&app, cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.prompt.as_ref().and_then(Prompt::target),
            Some(PromptTarget::AudioMatchText)
        );
    });
    cx.simulate_input("Warm piano, 柔らかな音色");
    cx.simulate_keystrokes("enter");
    app.read_with(cx, |this, _| {
        assert!(this.prompt.is_none());
        assert_eq!(this.reference_match.text_prompt, "Warm piano, 柔らかな音色");
        assert!(this.reference_match.source.is_none());
        assert!(this.reference_match.input_problem().is_none());
        assert_eq!(this.project(), &before);
        assert!(!this.session.can_undo());
    });
    paint(&app, cx);
    click("reference-match-start", cx);
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.reference_match.running.is_none());
        assert!(this.reference_match.comparison.is_none());
        assert!(
            this.reference_match.error.is_some(),
            "the missing model is reported by preparation"
        );
        assert_eq!(this.project(), &before);
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn changing_the_audio_target_cancels_obsolete_preparation(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        configure(this, cx);
        assert!(this.start_reference_match(cx));
        let snapshot = this
            .reference_match
            .running
            .as_ref()
            .unwrap()
            .snapshot
            .clone();
        let cancel = Arc::clone(&this.reference_match.running.as_ref().unwrap().cancel);
        this.change_reference_settings(|state| {
            state.objective = MatchObjective::ClapText;
            state.text_prompt = "Soft drums".into();
        });
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!this.match_snapshot_is_current(&snapshot));
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
fn escape_cancels_the_prompt_before_closing_the_audio_sheet(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        this.open_reference_match(cx);
        this.reference_match.objective = MatchObjective::ClapText;
        this.reference_match.text_prompt = "Piano".into();
    });
    paint(&app, cx);
    click("audio-match-prompt", cx);
    paint(&app, cx);
    cx.simulate_input("Discard this text");
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |this, _| {
        assert!(this.prompt.is_none());
        assert!(this.reference_match.open);
        assert_eq!(this.reference_match.text_prompt, "Piano");
    });
    cx.simulate_keystrokes("escape");
    app.read_with(cx, |this, _| assert!(!this.reference_match.open));
}

#[test]
fn clap_reference_requires_audio_but_text_does_not() {
    let mut state = ReferenceMatchState {
        objective: MatchObjective::ClapReference,
        model_directory: Some("model".into()),
        text_prompt: "Soft piano".into(),
        ..ReferenceMatchState::default()
    };
    assert_eq!(state.input_problem(), Some(Key::ReferenceMatchMissing));
    state.objective = MatchObjective::ClapText;
    assert!(state.input_problem().is_none());
    state.text_prompt.clear();
    assert_eq!(state.input_problem(), Some(Key::AudioMatchPromptRequired));
}

#[gpui::test]
fn audio_match_status_and_actions_stay_visible_in_short_windows(cx: &mut TestAppContext) {
    let (app, cx, _, clip) = with_a_clip(cx);
    app.update(cx, |this, cx| {
        this.session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        configure(this, cx);
        assert!(this.start_reference_match(cx));
    });
    cx.run_until_parked();
    let (snapshot, report) = app.read_with(cx, |this, _| {
        let result = this.reference_match.comparison.as_ref().unwrap();
        (result.snapshot.clone(), result.report.clone())
    });
    for language in [Language::English, Language::Japanese] {
        app.update(cx, |this, _| this.language = language);
        for (width, height) in [(900.0, 650.0), (640.0, 480.0)] {
            for (case, selector) in [
                "reference-match-input-problem",
                "reference-match-progress",
                "reference-match-progress",
                "reference-match-error",
                "reference-match-cancelled",
                "reference-match-complete",
                "reference-match-stale",
                "reference-match-preview-status",
            ]
            .into_iter()
            .enumerate()
            {
                app.update(cx, |this, _| {
                    let state = &mut this.reference_match;
                    state.objective = snapshot.objective;
                    state.comparison = None;
                    state.running = None;
                    state.error = None;
                    state.cancelled = false;
                    state.preview = None;
                    match case {
                        0 => state.objective = MatchObjective::ClapText,
                        1 | 2 => {
                            state.objective = MatchObjective::ClapReference;
                            let mut snapshot = snapshot.clone();
                            snapshot.objective = state.objective;
                            state.running = Some(MatchControl {
                                snapshot,
                                cancel: Arc::new(AtomicBool::new(case == 2)),
                                completed: Arc::new(AtomicUsize::new(2)),
                                fraction: Arc::new(AtomicU32::new(0.5_f32.to_bits())),
                                prepared: Arc::new(AtomicBool::new(false)),
                            });
                        }
                        3 => state.error = Some("Model loading failed. ".repeat(180)),
                        4 => state.cancelled = true,
                        5 | 6 => {
                            let mut snapshot = snapshot.clone();
                            if case == 6 {
                                snapshot.generation = snapshot.generation.wrapping_sub(1);
                            }
                            state.comparison = Some(MatchComparison {
                                snapshot,
                                report: report.clone(),
                            });
                        }
                        7 => {
                            state.preview = Some(MatchPreview {
                                selection: 1,
                                status: None,
                            });
                        }
                        _ => unreachable!(),
                    }
                });
                resize(&app, cx, size(px(width), px(height)));
                let panel = cx.debug_bounds("reference-match-panel").unwrap();
                let footer = cx.debug_bounds("reference-match-footer").unwrap();
                let status = cx.debug_bounds("reference-match-status").unwrap();
                let message = cx.debug_bounds(selector).unwrap();
                assert!(footer.top() >= panel.top());
                assert!(footer.bottom() <= panel.bottom());
                assert!(status.size.height <= px(80.0));
                assert!(message.top() >= status.top());
                assert!(message.top() < status.bottom());
                for action in [
                    if case == 1 || case == 2 {
                        "reference-match-cancel"
                    } else {
                        "reference-match-start"
                    },
                    "reference-match-apply",
                ] {
                    let bounds = cx.debug_bounds(action).unwrap();
                    assert!(bounds.top() >= status.bottom());
                    assert!(bounds.bottom() <= panel.bottom());
                }
            }
        }
    }
}

#[gpui::test]
fn text_matching_hides_an_old_reference_and_preview_status_clears_without_a_device(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        configure(this, cx);
        this.change_reference_settings(|state| state.objective = MatchObjective::ClapText);
    });
    // gpui retains debug bounds from earlier frames, so absence is checked before this
    // modal has ever painted its retained reference source in an acoustic mode.
    paint(&app, cx);
    assert!(cx.debug_bounds("reference-preview-source").is_none());
    app.update(cx, |this, cx| {
        this.preview_reference_match(0, cx);
        assert!(this.reference_match.preview.is_none());
    });
    click("audio-match-acoustic", cx);
    app.update(cx, |this, cx| this.preview_reference_match(0, cx));
    cx.run_until_parked();
    app.update(cx, |this, _| {
        this.poll_reference_match();
        assert!(this.reference_match.preview.is_none());
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn a_revision_change_during_preview_preparation_clears_the_pending_status(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        configure(this, cx);
        this.preview_reference_match(0, cx);
        assert!(this.reference_match.preview.is_some());
        this.session
            .add_default_instrument_track("New track")
            .unwrap();
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.reference_match.preview.is_none())
    });
}
