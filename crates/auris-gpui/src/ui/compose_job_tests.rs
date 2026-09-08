//! The composing task's lifetime as observed by the real window and document.

use auris_session::prelude::*;
use gpui::TestAppContext;

use crate::harness::open;
use crate::ui::compose_sheet::{another_take, song_dials};

fn short_song() -> SongSpec {
    SongSpec::parse(
        r#"
        title = "Asynchronous song"
        seed = 381
        form = "verse"
        ending = "none"
        [section.verse]
        bars = 1
        [[part]]
        name = "Written Bass"
        role = "bass"
        "#,
    )
    .unwrap()
}

#[gpui::test]
fn progress_appears_before_work_and_duplicate_requests_do_not_add_another_edit(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    let spec = short_song();
    let before = app.update(cx, |this, cx| {
        this.session
            .add_default_instrument_track("Previous song")
            .unwrap();
        this.session.forget_history();
        this.song_sheet = Some(song_dials(&spec));
        let before = this.project().clone();

        assert!(this.compose_spec(&spec, true, cx));
        assert!(
            this.compose_progress.is_some(),
            "progress is published before yielding"
        );
        assert!(
            this.song_sheet.is_some(),
            "the draft stays open until success"
        );
        assert_eq!(
            this.project(),
            &before,
            "the drawing thread has not written a score"
        );
        assert!(
            !this.compose_spec(&spec, true, cx),
            "one task owns composition"
        );
        assert!(
            !this.session.can_undo(),
            "starting a task is not a document edit"
        );
        before
    });

    cx.run_until_parked();
    app.update(cx, |this, _| {
        assert!(this.compose_progress.is_none());
        assert!(this.song_sheet.is_none());
        assert_eq!(this.project().name, spec.title);
        let written = this
            .project()
            .tracks
            .iter()
            .find(|track| track.name == "Written Bass")
            .and_then(|track| track.kind.as_instrument())
            .expect("the real composition reached the document");
        assert!(written.clips.iter().any(|clip| !clip.notes.is_empty()));
        assert_eq!(this.session.undo(), Some(auris_session::Edit::Compose));
        assert_eq!(this.project(), &before);
        assert!(
            !this.session.can_undo(),
            "composition and its mix form one undo step"
        );
    });
}

#[gpui::test]
fn another_take_completes_with_the_changed_seed_and_keeps_the_sheet_open(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let draft = app.update(cx, |this, cx| {
        let mut draft = song_dials(&short_song());
        another_take(&mut draft);
        this.song_sheet = Some(draft.clone());
        assert!(this.write_song_from_sheet(false, cx));
        assert!(this.compose_progress.is_some());
        draft
    });

    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.compose_progress.is_none());
        assert_eq!(this.song_sheet.as_ref(), Some(&draft));
        let written = SongSpec::parse(this.project().song_spec.as_deref().unwrap()).unwrap();
        assert_eq!(written.seed, draft.seed);
        assert_eq!(written.seed, short_song().seed + 1);
        assert!(
            this.project()
                .tracks
                .iter()
                .any(|track| track.name == "Written Bass")
        );
    });
}

#[gpui::test]
fn an_unavailable_source_finishes_with_the_original_draft_and_document_intact(
    cx: &mut TestAppContext,
) {
    let (app, cx) = open(cx);
    let (before, draft, dirty) = app.update(cx, |this, cx| {
        this.session
            .add_default_instrument_track("Keep me")
            .unwrap();
        this.session.forget_history();
        let mut spec = short_song();
        let path = std::env::temp_dir()
            .join(format!("auris-compose-job-missing-{}", std::process::id()))
            .join("never-created.sf2");
        assert!(!path.exists());
        spec.parts[0].source = Some(PartSource::SoundFont {
            path,
            bank: 0,
            patch: 0,
        });
        let draft = song_dials(&spec);
        this.song_sheet = Some(draft.clone());
        let before = this.project().clone();
        let dirty = this.session.is_dirty();
        assert!(
            this.write_song_from_sheet(true, cx),
            "asset validation happens in the task"
        );
        assert!(this.compose_progress.is_some());
        (before, draft, dirty)
    });

    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(
            this.compose_progress.is_none(),
            "failed work must release the busy state"
        );
        assert_eq!(this.song_sheet.as_ref(), Some(&draft));
        assert_eq!(this.project(), &before);
        assert_eq!(this.session.is_dirty(), dirty);
        assert!(!this.session.can_undo());
        assert!(this.status_failed);
        assert!(
            this.prompt.is_some(),
            "the source error remains reviewable on the sheet"
        );
    });
}

#[gpui::test]
fn stale_balancing_keeps_the_current_document_and_song_draft_open(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let draft = app.update(cx, |this, cx| {
        let spec = short_song();
        let draft = song_dials(&spec);
        this.song_sheet = Some(draft.clone());
        assert!(this.compose_spec(&spec, true, cx));
        draft
    });

    // Advance one executor task at a time. This stops after adoption has captured a detached
    // balance job and before its continuation can accept a measurement, without a timing race.
    let executor = cx.executor();
    for step in 0..128 {
        if app.read_with(cx, |this, _| {
            this.compose_progress
                .as_ref()
                .is_some_and(|progress| progress.stage == auris_i18n::Key::ComposeProgressBalancing)
        }) {
            break;
        }
        assert!(step < 127, "the composition never started balancing");
        assert!(executor.tick(), "the composition parked before balancing");
    }
    let newer = app.update(cx, |this, _| {
        assert_eq!(this.project().name, short_song().title);
        this.session
            .add_default_instrument_track("Newer work during balance")
            .unwrap();
        this.project().clone()
    });

    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.compose_progress.is_none());
        assert_eq!(this.project(), &newer);
        assert_eq!(
            this.song_sheet.as_ref(),
            Some(&draft),
            "stale work cannot close the draft"
        );
        assert!(this.status_failed);
        assert!(this.prompt.is_some());
    });
}

#[gpui::test]
fn a_score_finished_for_an_older_document_does_not_replace_a_newer_edit(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let newer = app.update(cx, |this, cx| {
        let spec = short_song();
        this.song_sheet = Some(song_dials(&spec));
        assert!(this.compose_spec(&spec, true, cx));
        assert!(this.compose_progress.is_some());
        // The executor has not run yet. An external edit invalidates the captured document
        // revision even if the composer happens to finish its score immediately afterwards.
        this.session
            .add_default_instrument_track("Newer work")
            .unwrap();
        this.project().clone()
    });

    cx.run_until_parked();
    app.update(cx, |this, _| {
        assert!(this.compose_progress.is_none());
        assert_eq!(this.project(), &newer);
        assert!(this.song_sheet.is_some());
        assert_ne!(this.session.undo(), Some(auris_session::Edit::Compose));
    });
}
