//! Search gestures and task lifetime, observed through the real sheet and document.

use auris_i18n::Language;
use auris_session::composition_search::{SearchMethod, search_composition};
use auris_session::prelude::{Note, Ticks};
use gpui::{
    Entity, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point, px, size,
};

use super::*;
use crate::harness::{click, open, paint, resize};
use crate::ui::compose_sheet::song_dials;

fn short_song() -> SongSpec {
    SongSpec::parse(
        r#"
        title = "Search gesture"
        seed = 381
        form = "verse"
        ending = "none"
        [section.verse]
        bars = 1
        intensity = 0.5
        [[part]]
        name = "Written Bass"
        role = "bass"
        density = 0.5
        "#,
    )
    .unwrap()
}

fn prepare(app: &Entity<AurisApp>, cx: &mut VisualTestContext) {
    app.update(cx, |this, _| {
        this.session
            .add_default_instrument_track("Previous song")
            .unwrap();
        this.session.forget_history();
        this.song_sheet = Some(song_dials(&short_song()));
        this.song_advanced = false;
    });
    paint(app, cx);
    click("song-search-toggle", cx);
    paint(app, cx);
    app.update(cx, |this, _| {
        assert!(this.composition_search.open);
        this.composition_search.settings.attempt_budget = 8;
    });
    paint(app, cx);
}

fn completed(this: &AurisApp) -> CompletedSearch {
    let song = song_spec(this.song_sheet.as_ref().unwrap());
    let settings = this.composition_search.settings.clone();
    let result = search_composition(&settings.to_request(&song), || false).unwrap();
    CompletedSearch {
        snapshot: SearchSnapshot {
            revision: this.session.revision(),
            song,
            settings,
        },
        result,
    }
}

fn reveal(app: &Entity<AurisApp>, cx: &mut VisualTestContext, selector: &'static str) {
    let body = cx.debug_bounds("song-sheet-body").unwrap();
    let control = cx.debug_bounds(selector).unwrap();
    if control.top() < body.top() || control.bottom() > body.bottom() {
        cx.simulate_event(ScrollWheelEvent {
            position: body.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), body.top() + px(8.0) - control.top())),
            ..Default::default()
        });
        paint(app, cx);
    }
    let control = cx.debug_bounds(selector).unwrap();
    assert!(
        control.top() >= body.top() && control.bottom() <= body.bottom(),
        "the control can be reached in the scrolling sheet: {selector}"
    );
}

#[gpui::test]
fn search_buttons_run_the_selected_request_without_editing_the_document(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    prepare(&app, cx);
    click("song-search-method-0", cx);
    paint(&app, cx);
    click("song-search-target-12", cx);
    paint(&app, cx);
    let (before, revision, expected) = app.read_with(cx, |this, _| {
        assert_eq!(
            this.composition_search.settings.algorithm,
            SearchMethod::Random
        );
        assert_eq!(this.composition_search.settings.target_notes_per_bar, 12.0);
        (
            this.project().clone(),
            this.session.revision(),
            completed(this).result,
        )
    });
    reveal(&app, cx, "song-search-start");
    click("song-search-start", cx);
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        let state = &this.composition_search;
        assert!(state.running.is_none());
        assert_eq!(state.result.as_ref().unwrap().result, expected);
        assert_eq!(this.project(), &before);
        assert_eq!(this.session.revision(), revision);
        assert!(!this.session.can_undo());
        assert!(this.song_sheet.is_some());
        let status = this.song_search_view_status();
        assert_eq!(status.completed, 8);
        assert!(status.best_density.is_some());
        assert!(status.can_apply);
    });
}

#[gpui::test]
fn a_run_publishes_progress_and_waits_for_a_stale_worker_before_restarting(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    prepare(&app, cx);
    app.update(cx, |this, cx| {
        assert!(this.start_song_search(cx));
        let generation = this.composition_search.generation;
        assert!(this.song_search_view_status().running);
        assert_eq!(this.song_search_view_status().completed, 0);
        assert!(!this.start_song_search(cx));
        assert_eq!(this.composition_search.generation, generation);
        assert!(!this.session.can_undo());
        this.song_sheet.as_mut().unwrap().seed += 1;
        this.poll_song_search();
        assert!(this.song_search_view_status().cancelling);
        assert!(this.composition_search.running.is_some());
        assert!(this.composition_search.generation > generation);
        let cancelled_generation = this.composition_search.generation;
        this.poll_song_search();
        assert_eq!(this.composition_search.generation, cancelled_generation);
        assert!(
            !this.start_song_search(cx),
            "the cancelled worker still owns the slot"
        );
    });
    cx.run_until_parked();
    app.update(cx, |this, cx| {
        assert!(this.composition_search.running.is_none());
        assert!(this.composition_search.result.is_none());
        assert!(this.start_song_search(cx));
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert_eq!(this.song_search_view_status().completed, 8);
        assert_eq!(
            this.composition_search
                .result
                .as_ref()
                .unwrap()
                .snapshot
                .song
                .seed,
            short_song().seed + 1
        );
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn cancelling_before_the_worker_runs_returns_an_empty_partial_result(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    prepare(&app, cx);
    app.update(cx, |this, cx| {
        assert!(this.start_song_search(cx));
        this.cancel_song_search(cx);
        assert!(this.song_search_view_status().cancelling);
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        let result = &this.composition_search.result.as_ref().unwrap().result;
        assert_eq!(result.termination, TerminationReason::Cancelled);
        assert!(result.history.is_empty());
        assert!(result.best.is_none());
        assert!(!this.song_search_view_status().can_apply);
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn cancel_and_close_buttons_signal_the_worker_and_running_settings_are_locked(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    prepare(&app, cx);
    let cancel = app.update(cx, |this, _| {
        let snapshot = completed(this).snapshot;
        let cancel = Arc::new(AtomicBool::new(false));
        this.composition_search.running = Some(RunningSearch {
            generation: this.composition_search.generation,
            snapshot,
            cancel: Arc::clone(&cancel),
            completed: Arc::new(AtomicUsize::new(2)),
            best_density: Arc::new(AtomicU64::new(5.0f64.to_bits())),
        });
        cancel
    });
    paint(&app, cx);
    click("song-search-method-0", cx);
    app.read_with(cx, |this, _| {
        assert_eq!(
            this.composition_search.settings.algorithm,
            SearchMethod::HillClimb
        );
        assert_eq!(this.song_search_view_status().completed, 2);
        assert_eq!(this.song_search_view_status().best_density, Some(5.0));
    });
    reveal(&app, cx, "song-search-cancel");
    click("song-search-cancel", cx);
    assert!(cancel.load(Ordering::Relaxed));
    let close_cancel = app.update(cx, |this, _| {
        assert!(this.song_search_view_status().cancelling);
        // A fresh controlled token lets closing the sheet demonstrate its own cancellation.
        let close_cancel = Arc::new(AtomicBool::new(false));
        this.composition_search.running.as_mut().unwrap().cancel = Arc::clone(&close_cancel);
        close_cancel
    });
    paint(&app, cx);
    click("song-sheet-cancel", cx);
    assert!(close_cancel.load(Ordering::Relaxed));
    app.update(cx, |this, _| {
        assert!(this.song_sheet.is_none());
        assert!(this.composition_search.running.is_some());
        // This fixture has no worker to return and release its cancelled slot.
        this.composition_search.running = None;
    });
}

#[gpui::test]
fn applying_a_partial_winner_uses_the_exact_notes_and_one_undo_restores_the_document(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    prepare(&app, cx);
    let expected_notes = vec![Note::new(109, Ticks(137), Ticks(411))];
    let before = app.update(cx, |this, _| {
        let mut completed = completed(this);
        let cancel = AtomicBool::new(false);
        let request = completed
            .snapshot
            .settings
            .to_request(&completed.snapshot.song);
        completed.result = search_composition_with_progress(
            &request,
            || cancel.load(Ordering::Relaxed),
            |attempt| {
                if attempt.candidate.id == 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .unwrap();
        assert_eq!(completed.result.termination, TerminationReason::Cancelled);
        assert_eq!(completed.result.history.len(), 2);
        // A distinctive evaluated artifact catches any accidental regeneration during adoption.
        completed.result.best.as_mut().unwrap().score.tracks[0].clips[0].notes =
            expected_notes.clone();
        this.composition_search.result = Some(completed);
        assert!(this.song_search_view_status().can_apply);
        this.project().clone()
    });
    paint(&app, cx);
    reveal(&app, cx, "song-search-apply");
    click("song-search-apply", cx);
    cx.run_until_parked();
    app.update(cx, |this, _| {
        assert!(this.compose_progress.is_none());
        assert!(this.song_sheet.is_none());
        let written = this
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "Written Bass")
            .and_then(|track| track.kind.as_instrument())
            .unwrap();
        assert_eq!(written.clips[0].notes, expected_notes);
        assert_eq!(this.session.undo(), Some(auris_session::Edit::Compose));
        assert_eq!(this.project(), &before);
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn changing_the_sheet_settings_or_document_prevents_stale_adoption(cx: &mut TestAppContext) {
    for change in 0..3 {
        let (app, cx) = open(cx);
        prepare(&app, cx);
        let before = app.update(cx, |this, cx| {
            this.composition_search.result = Some(completed(this));
            match change {
                0 => this.song_sheet.as_mut().unwrap().seed += 1,
                1 => this.composition_search.settings.target_notes_per_bar += 1.0,
                _ => {
                    this.session
                        .add_default_instrument_track("New edit")
                        .unwrap();
                }
            }
            let before = this.project().clone();
            assert!(this.song_search_view_status().stale);
            assert!(!this.song_search_view_status().can_apply);
            assert!(!this.apply_song_search(cx));
            assert_eq!(
                this.composition_search.error.as_deref(),
                Some(this.t(Key::SongSearchChanged))
            );
            before
        });
        paint(&app, cx);
        reveal(&app, cx, "song-search-apply");
        click("song-search-apply", cx);
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(this.compose_progress.is_none());
            assert_eq!(this.project(), &before);
            assert!(this.song_sheet.is_some());
        });
    }
}

#[gpui::test]
fn closing_and_reopening_the_sheet_keeps_the_new_run_safe_from_a_late_old_result(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    prepare(&app, cx);
    let expected = app.update(cx, |this, cx| {
        assert!(this.start_song_search(cx));
        let old_cancel = Arc::clone(&this.composition_search.running.as_ref().unwrap().cancel);
        // Keep closing and reopening in one turn: pointer event dispatch may yield to workers.
        // The controlled-running gesture test above separately checks the actual close button.
        this.composition_search.dismiss();
        this.song_sheet = None;
        assert!(old_cancel.load(Ordering::Relaxed));
        this.open_song_sheet();
        this.song_sheet = Some(song_dials(&short_song()));
        this.toggle_song_search(cx);
        this.composition_search.settings.search_seed = 917;
        this.composition_search.settings.attempt_budget = 8;
        let expected = completed(this).result;
        assert!(this.composition_search.running.is_some());
        assert!(
            !this.start_song_search(cx),
            "closing cannot overlap worker attempts"
        );
        expected
    });
    cx.run_until_parked();
    app.update(cx, |this, cx| {
        assert!(this.composition_search.running.is_none());
        assert!(
            this.composition_search.result.is_none(),
            "the closed sheet's result is discarded"
        );
        assert!(this.start_song_search(cx));
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        let result = this.composition_search.result.as_ref().unwrap();
        assert_eq!(result.snapshot.settings.search_seed, 917);
        assert_eq!(result.result, expected);
        assert!(this.composition_search.error.is_none());
        assert!(!this.session.can_undo());
    });
}

#[gpui::test]
fn search_controls_remain_reachable_by_scrolling_a_small_window_in_both_languages(
    cx: &mut TestAppContext,
) {
    for language in [Language::English, Language::Japanese] {
        let (app, cx) = open(cx);
        resize(&app, cx, size(px(640.0), px(480.0)));
        app.update(cx, |this, _| this.language = language);
        prepare(&app, cx);
        reveal(&app, cx, "song-search-method-0");
        click("song-search-method-0", cx);
        paint(&app, cx);
        reveal(&app, cx, "song-search-target-12");
        click("song-search-target-12", cx);
        paint(&app, cx);
        reveal(&app, cx, "song-search-start");
        click("song-search-start", cx);
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.composition_search.settings.algorithm,
                SearchMethod::Random
            );
            assert_eq!(this.composition_search.settings.target_notes_per_bar, 12.0);
            assert_eq!(this.song_search_view_status().completed, 8);
            assert!(this.song_search_view_status().can_apply);
            assert!(!this.session.can_undo());
        });
    }
}
