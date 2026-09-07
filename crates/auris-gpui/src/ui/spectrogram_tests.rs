//! Worker publication and cache lifetime through the real headless window.

use std::path::Path;

use gpui::TestAppContext;

use super::*;
use crate::harness::{open, paint};
use crate::ui::context_menu::MenuCommand;

#[gpui::test]
fn rendered_tracks_and_project_refresh_after_edits_and_reset(cx: &mut TestAppContext) {
    let (app, cx, track, clip) = crate::harness::with_a_clip(cx);
    app.update(cx, |this, cx| {
        this.session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        let revision = this.session.revision();
        this.run_menu_command(
            MenuCommand::SetTrackSpectrogram {
                track,
                enabled: true,
            },
            cx,
        );
        this.run_menu_command(MenuCommand::SetProjectSpectrogram(true), cx);
        assert_eq!(this.session.revision(), revision);
    });
    cx.run_until_parked();
    paint(&app, cx);
    app.update(cx, |this, cx| {
        assert!(this.rendered_spectrum_paint(Some(track)).image.is_some());
        assert!(this.rendered_spectrum_paint(None).image.is_some());
        assert!(this.spectrograms.rendering.is_none());
        this.session.move_clip(clip, Ticks::QUARTER).unwrap();
        assert!(this.rendered_spectrum_paint(Some(track)).image.is_none());
        assert!(this.rendered_spectrum_paint(None).image.is_none());
        this.poll_spectrograms(cx);
        assert!(this.spectrograms.rendering.is_some());
        this.new_project();
        assert!(!this.spectrograms.project_enabled);
        assert!(this.spectrograms.rendered.is_empty());
    });
    cx.run_until_parked();
    app.read_with(cx, |this, _| {
        assert!(this.spectrograms.rendering.is_none());
        assert!(this.spectrograms.rendered.is_empty());
        assert!(this.spectrogram_tracks.is_empty());
    });
}

#[gpui::test]
fn project_spectrum_can_be_closed_without_changing_the_document(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    app.update(cx, |this, cx| {
        this.run_menu_command(MenuCommand::SetProjectSpectrogram(true), cx)
    });
    cx.run_until_parked();
    paint(&app, cx);
    let revision = app.read_with(cx, |this, _| this.session.revision());
    crate::harness::click("close-project-spectrogram", cx);
    app.read_with(cx, |this, _| {
        assert!(!this.spectrograms.project_enabled);
        assert_eq!(this.session.revision(), revision);
    });
}

/// Use the decoded half of audio import so the fixture needs no file or audio device.
fn import_tone(app: &mut AurisApp, name: &str, frames: usize) -> (TrackId, ClipId, SourceId) {
    let rate = app.project().sample_rate;
    let samples = (0..frames)
        .map(|frame| (std::f64::consts::TAU * 1_000.0 * frame as f64 / rate).sin() as f32 * 0.5)
        .collect();
    let buffer = AudioBuffer::from_planar(vec![samples], rate).unwrap();
    let clip = app
        .session
        .place_audio(Path::new(name), buffer, Ticks::ZERO)
        .expect("decoded audio is installed on its own track");
    let track = app.session.track_of_clip(clip).unwrap();
    let source = app.project().audio_clip(clip).unwrap().source;
    (track, clip, source)
}

#[gpui::test]
fn enabling_an_imported_track_publishes_its_image_from_the_worker(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let (track, _, source) = app.update(cx, |this, _| import_tone(this, "spectrum.wav", 8_192));
    paint(&app, cx);

    app.update(cx, |this, cx| {
        assert!(this.spectrograms.get(source, &this.session).is_none());
        this.set_track_spectrogram(track, true, cx);
        assert!(this.spectrograms.pending.is_some());
        assert!(
            this.spectrograms.get(source, &this.session).is_none(),
            "the command schedules work instead of analysing during the UI update"
        );
    });
    cx.run_until_parked();

    app.read_with(cx, |this, _| {
        let image = this
            .spectrograms
            .get(source, &this.session)
            .expect("the worker publishes the visible source");
        assert_eq!(image.frames, 8_192);
        assert!(image.high_hz > image.low_hz);
        assert!(this.spectrograms.pending.is_none());
        assert_eq!(this.spectrograms.entries.len(), 1);
    });
}

#[gpui::test]
fn an_empty_source_finishes_without_an_image_or_repeated_work(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let (track, _, source) = app.update(cx, |this, _| import_tone(this, "empty.wav", 0));
    paint(&app, cx);
    app.update(cx, |this, cx| this.set_track_spectrogram(track, true, cx));
    cx.run_until_parked();
    app.update(cx, |this, cx| {
        assert!(this.spectrograms.is_complete(source, &this.session));
        assert!(this.spectrograms.get(source, &this.session).is_none());
        this.poll_spectrograms(cx);
        assert!(this.spectrograms.pending.is_none());
    });
}

#[gpui::test]
fn a_new_project_rejects_pending_results_and_can_analyse_its_own_source(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let (track, _, cached_source) =
        app.update(cx, |this, _| import_tone(this, "cached.wav", 8_192));
    paint(&app, cx);
    app.update(cx, |this, cx| this.set_track_spectrogram(track, true, cx));
    cx.run_until_parked();

    let fresh_source = app.update(cx, |this, cx| {
        assert!(
            this.spectrograms
                .get(cached_source, &this.session)
                .is_some()
        );
        let (pending_track, _, pending_source) = import_tone(this, "pending.wav", 12_288);
        this.set_track_spectrogram(pending_track, true, cx);
        let pending = this
            .spectrograms
            .pending
            .expect("the second source is queued");
        assert_eq!(pending.0, pending_source);
        let old_job = this.session.spectrogram_job(pending_source).unwrap();
        let generation = this.spectrograms.generation;

        this.new_project();
        assert!(this.spectrogram_tracks.is_empty());
        assert!(this.spectrograms.entries.is_empty());
        assert_ne!(this.spectrograms.generation, generation);
        assert!(!this.session.spectrogram_job_is_current(&old_job));
        assert!(
            this.spectrograms
                .get(cached_source, &this.session)
                .is_none()
        );

        let (fresh_track, _, fresh_source) = import_tone(this, "fresh.wav", 16_384);
        this.set_track_spectrogram(fresh_track, true, cx);
        assert_eq!(
            this.spectrograms.pending,
            Some(pending),
            "a reset keeps the single worker slot until its old job returns"
        );
        fresh_source
    });
    cx.run_until_parked();

    app.read_with(cx, |this, _| {
        assert!(this.spectrograms.pending.is_none());
        assert_eq!(
            this.spectrograms.entries.len(),
            1,
            "neither an old cached image nor an old pending result enters the new project"
        );
        let image = this
            .spectrograms
            .get(fresh_source, &this.session)
            .expect("finishing an obsolete job schedules the new project's visible source");
        assert_eq!(image.frames, 16_384);
    });
}

#[gpui::test]
fn display_toggles_and_clip_edits_reuse_the_source_image(cx: &mut TestAppContext) {
    let (app, cx) = open(cx);
    let (track, clip, source) = app.update(cx, |this, _| import_tone(this, "edits.wav", 8_192));
    paint(&app, cx);
    app.update(cx, |this, cx| this.set_track_spectrogram(track, true, cx));
    cx.run_until_parked();

    app.update(cx, |this, cx| {
        let original = this.spectrograms.get(source, &this.session).unwrap();
        this.set_track_spectrogram(track, false, cx);
        this.session.set_clip_gain(clip, -6.0).unwrap();
        this.session.set_clip_fades(clip, 128, 256).unwrap();
        this.session.move_clip(clip, Ticks(240)).unwrap();
        this.session.trim_clip_start(clip, Ticks(256)).unwrap();
        this.session.set_clip_source_bpm(clip, Some(90.0)).unwrap();
        this.session.set_clip_follows_tempo(clip, true).unwrap();
        this.set_track_spectrogram(track, true, cx);

        let reused = this.spectrograms.get(source, &this.session).unwrap();
        assert!(
            Arc::ptr_eq(&original, &reused),
            "clip presentation and timing use the same source analysis"
        );
        assert!(this.spectrograms.pending.is_none());
        assert_eq!(this.spectrograms.entries.len(), 1);
    });
}
