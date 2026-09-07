//! Automatic acoustic kit measurements and their explicit application to a drum track.

mod state;

pub(crate) use state::DrumAnalysisState;
use state::{CHECK_INTERVAL, Outcome};

use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, Instant};

use auris_i18n::Key;
use auris_session::prelude::*;
use gpui::{AnyElement, Context, IntoElement, div, prelude::*};

use crate::app::AurisApp;
use crate::ui::context_menu::MenuCommand;
use crate::ui::widgets::{ButtonStyle, button};

/// The shared label for a measured sound or an independent kit writer.
pub(crate) fn role_key(role: DrumRole) -> Key {
    match role {
        DrumRole::Kick => Key::RoleKick,
        DrumRole::Snare => Key::RoleSnare,
        DrumRole::ClosedHat => Key::DrumClosedHat,
        DrumRole::OpenHat => Key::DrumOpenHat,
        DrumRole::Crash => Key::RoleCrash,
        DrumRole::Tom => Key::DrumTom,
    }
}

impl AurisApp {
    /// Requests another attempt for this sound, without competing with an active worker.
    pub(crate) fn begin_drum_analysis(&mut self, track: TrackId, cx: &mut Context<Self>) {
        self.analysis_panel = true;
        if let Some(source) = self.session.drum_analysis_source_key(track) {
            self.drum_analysis.retry(track, source, Instant::now());
            self.poll_drum_analysis(cx);
        }
    }

    fn observe_drum_sources(&mut self, now: Instant) {
        let sources = self
            .project()
            .tracks
            .iter()
            .filter_map(|track| {
                self.session
                    .drum_analysis_source_key(track.id)
                    .map(|key| (track.id, key))
            })
            .collect();
        self.drum_analysis.observe(sources, now);
    }

    /// Debounces sound changes and runs at most one isolated probe in the background.
    pub(crate) fn poll_drum_analysis(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self
            .drum_analysis
            .checked_at
            .is_none_or(|checked| now.duration_since(checked) >= CHECK_INTERVAL)
        {
            self.observe_drum_sources(now);
        }
        // Snapshotting a hosted source belongs to its owning thread and happens only once
        // an attempt is due. Recording and pointer edits get to finish first.
        if self.session.is_recording()
            || self.drag.is_some()
            || self.choosing_export
            || self
                .export
                .as_ref()
                .is_some_and(|export| export.result.is_none())
        {
            return;
        }
        let Some(job) = self.drum_analysis.start_next(now) else {
            return;
        };
        let request = match self
            .session
            .drum_probe_request(job.track, &Default::default())
        {
            Ok(request) => request,
            Err(error) => {
                self.drum_analysis.take_finished(&job.cancel);
                self.drum_analysis.finish(
                    job.track,
                    job.source,
                    Outcome::Failed(error.to_string()),
                );
                return;
            }
        };
        let cancel = job.cancel;
        cx.spawn(async move |this, cx| {
            let control = Arc::clone(&cancel);
            let report = cx
                .background_executor()
                .spawn(async move {
                    auris_session::run_drum_probe_isolated(
                        &request,
                        &control,
                        Duration::from_secs(600),
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_drum_analysis(&cancel, report);
                cx.notify();
            });
        })
        .detach();
    }

    /// Accepts only the active job and verifies its sound snapshot once, on completion.
    ///
    /// Taking a fresh snapshot may call a hosted instrument's main-thread state API, so this
    /// check happens on the owning thread rather than while rendering the inspector.
    fn finish_drum_analysis(
        &mut self,
        cancel: &Arc<AtomicBool>,
        report: Result<auris_session::DrumKitAnalysis, SessionError>,
    ) {
        self.observe_drum_sources(Instant::now());
        let Some((track, source)) = self.drum_analysis.take_finished(cancel) else {
            return;
        };
        let outcome = match report {
            Ok(report) => match self
                .session
                .drum_probe_request(report.track, &report.options)
            {
                Ok(current)
                    if current.track == track
                        && current.source_fingerprint == report.source_fingerprint
                        && current.sample_rate == report.sample_rate
                        && current.bpm == report.bpm =>
                {
                    Outcome::Ready(Box::new(report))
                }
                Ok(_) => Outcome::Failed(self.t(Key::DrumAnalysisObsolete).to_string()),
                Err(error) => Outcome::Failed(error.to_string()),
            },
            Err(error) => Outcome::Failed(error.to_string()),
        };
        self.drum_analysis.finish(track, source, outcome);
    }

    /// Cancels this track's queued or active attempt without requeuing the unchanged sound.
    pub(crate) fn cancel_drum_analysis(&mut self, track: TrackId) {
        self.observe_drum_sources(Instant::now());
        self.drum_analysis.cancel(track);
    }

    /// A document replacement invalidates all proposals, even when track IDs are reused.
    pub(crate) fn reset_drum_analysis(&mut self) {
        self.drum_analysis.reset();
    }

    /// Applies the measured proposal only after the user chooses its action.
    pub(crate) fn apply_measured_drums(&mut self, track: TrackId, remap_generated: bool) {
        let Some(report) = self.drum_analysis.report(track).cloned() else {
            return;
        };
        match self.session.apply_drum_map(&report, remap_generated) {
            Ok(_) => self.set_status(self.t(if remap_generated {
                Key::EditApplyDrumMap
            } else {
                Key::MenuUseDrumMapForGeneration
            })),
            Err(error) => self.set_failed_status(error.to_string()),
        }
    }

    /// Shows numeric fitness and the measured spectrum and duration of proposed sounds.
    pub(crate) fn drum_analysis_rows(
        &self,
        track: TrackId,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if !self
            .project()
            .track(track)
            .is_some_and(|track| track.kind.is_drum())
        {
            return Vec::new();
        }
        let theme = self.theme.clone();
        let mut rows = Vec::new();
        let (status, label, action) = match self.drum_analysis.outcome(track) {
            Some(Outcome::Running) => (
                Key::DrumAnalysisRunning,
                Key::MenuCancelDrumAnalysis,
                MenuCommand::CancelDrumAnalysis(track),
            ),
            Some(Outcome::Ready(_)) => (
                Key::DrumAnalysisReady,
                Key::MenuAnalyzeDrums,
                MenuCommand::AnalyzeDrums(track),
            ),
            Some(Outcome::Failed(_)) => (
                Key::DrumAnalysisFailed,
                Key::MenuRetryDrumAnalysis,
                MenuCommand::AnalyzeDrums(track),
            ),
            Some(Outcome::Cancelled) => (
                Key::DrumAnalysisCancelled,
                Key::MenuRetryDrumAnalysis,
                MenuCommand::AnalyzeDrums(track),
            ),
            Some(Outcome::Pending) | None => (
                Key::DrumAnalysisQueued,
                Key::MenuCancelDrumAnalysis,
                MenuCommand::CancelDrumAnalysis(track),
            ),
        };
        rows.push(
            div()
                .id("drum-analysis-status")
                .text_xs()
                .text_color(theme.text_muted)
                .child(self.t(status))
                .into_any_element(),
        );
        if let Some(Outcome::Failed(error)) = self.drum_analysis.outcome(track) {
            rows.push(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        rows.push(
            button(
                "drum-measure",
                self.t(label),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.run_menu_command(action.clone(), cx);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        let Some(report) = self.drum_analysis.report(track) else {
            return rows;
        };
        rows.push(
            button(
                "drum-apply-future",
                self.t(Key::MenuUseDrumMapForGeneration),
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.run_menu_command(MenuCommand::UseDrumMapForGeneration(track), cx);
                    cx.notify();
                }),
            )
            .into_any_element(),
        );
        if !report.proposed_map.voices.is_empty() {
            rows.push(
                button(
                    "drum-apply",
                    self.t(Key::MenuApplyDrumMap),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                        this.run_menu_command(MenuCommand::ApplyDrumMap(track), cx);
                        cx.notify();
                    }),
                )
                .into_any_element(),
            );
        }
        for role in DrumRole::ALL {
            let label = self.t(role_key(role));
            let detail = report
                .proposed_map
                .voices
                .get(&role)
                .and_then(|note| report.voices.iter().find(|voice| voice.note == *note))
                .map(|voice| {
                    let count = voice.samples.len().max(1) as f64;
                    let centroid = voice
                        .samples
                        .iter()
                        .map(|sample| sample.acoustics.spectrum.centroid_hz)
                        .sum::<f64>()
                        / count;
                    let duration = voice
                        .samples
                        .iter()
                        .map(|sample| sample.acoustics.energy_duration_seconds)
                        .sum::<f64>()
                        * 1000.0
                        / count;
                    format!(
                        "{} · {} {:.2} · {:.0} Hz · {:.0} ms",
                        voice.note,
                        self.t(Key::DrumFit),
                        voice.fitness.get(&role).copied().unwrap_or(0.0),
                        centroid,
                        duration
                    )
                })
                .unwrap_or_else(|| self.t(Key::DrumNoMatch).to_string());
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(label)
                    .child(detail)
                    .into_any_element(),
            );
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::{DrumKitAnalysis, DrumScanOptions};
    use gpui::TestAppContext;
    use std::sync::atomic::Ordering;

    use crate::harness::{click, open, paint};

    #[gpui::test]
    fn analysis_controls_follow_the_track_kind_not_the_loaded_instrument(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (melodic, drum) = app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            let melodic = this
                .session
                .add_instrument_track("Melodic", "auris.synth.drumkit")
                .unwrap();
            let drum = this
                .session
                .add_drum_track("Drums", "auris.synth.chiptune")
                .unwrap();
            this.select_track(melodic);
            (melodic, drum)
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("drum-measure").is_none());
        app.update(cx, |this, cx| {
            assert!(this.drum_analysis_rows(melodic, cx).is_empty());
            this.select_track(drum);
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("drum-measure").is_none());
        cx.dispatch_action(crate::actions::OpenDrumAnalysis);
        paint(&app, cx);
        assert!(cx.debug_bounds("drum-measure").is_some());
    }

    fn measure_one(this: &mut AurisApp, track: TrackId, note: u8) -> DrumKitAnalysis {
        // A known built-in instrument keeps this fixture bounded; the window uses a process.
        this.session
            .analyze_drum_kit(
                track,
                &DrumScanOptions {
                    notes: vec![note],
                    velocities: vec![1.0],
                    repetitions: 1,
                    seconds_per_note: 1.0,
                    ..Default::default()
                },
            )
            .unwrap()
    }

    fn start_fixture(this: &mut AurisApp, track: TrackId) -> Arc<AtomicBool> {
        let source = this.session.drum_analysis_source_key(track).unwrap();
        let now = Instant::now();
        this.drum_analysis.retry(track, source, now);
        let job = this.drum_analysis.start_next(now).unwrap();
        assert_eq!(job.track, track);
        job.cancel
    }

    fn accept_fixture(this: &mut AurisApp, report: DrumKitAnalysis) {
        let track = report.track;
        let cancel = start_fixture(this, track);
        this.finish_drum_analysis(&cancel, Ok(report));
        assert!(this.drum_analysis.report(track).is_some());
    }

    #[gpui::test]
    fn future_only_button_accepts_empty_and_incomplete_maps_without_rewriting_clips(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            // Other window tests save dock visibility in the shared test config directory.
            // This gesture needs the inspector open in its usual position and size.
            this.panels = crate::dock::PanelLayout::default();
        });
        for measured_note in [0, 35] {
            let (track, clip, original, map) = app.update(cx, |this, _| {
                this.panels = crate::dock::PanelLayout::default();
                let track = this
                    .session
                    .add_drum_track("Kit", "auris.synth.drumkit")
                    .unwrap();
                this.session
                    .stamp_named_progression("axis", Ticks::ZERO, 4)
                    .unwrap();
                let clip = this
                    .session
                    .generate_clip(
                        track,
                        Ticks::ZERO,
                        Ticks::QUARTER * 4,
                        ClipRecipe::new(ClipPreset::Hat, 2),
                    )
                    .unwrap();
                let original = this.session.midi_clip(clip).unwrap().clone();
                assert!(!original.notes.is_empty());
                let report = measure_one(this, track, measured_note);
                let map = report.proposed_map.clone();
                assert!(!map.voices.contains_key(&DrumRole::ClosedHat));
                if measured_note == 0 {
                    assert!(map.voices.is_empty());
                } else {
                    assert_eq!(map.voices.get(&DrumRole::Kick), Some(&35));
                }
                this.select_track(track);
                accept_fixture(this, report);
                (track, clip, original, map)
            });
            paint(&app, cx);
            cx.dispatch_action(crate::actions::OpenDrumAnalysis);
            paint(&app, cx);
            click("drum-apply-future", cx);
            app.update(cx, |this, _| {
                let instrument = this
                    .project()
                    .track(track)
                    .unwrap()
                    .kind
                    .as_instrument()
                    .unwrap();
                assert_eq!(
                    DrumMap::load(&instrument.instrument_state),
                    Some(map.clone())
                );
                assert_eq!(this.session.midi_clip(clip), Some(&original));
                let future = this
                    .session
                    .generate_clip(
                        track,
                        Ticks::QUARTER * 4,
                        Ticks::QUARTER * 4,
                        ClipRecipe::new(ClipPreset::Drums, 3),
                    )
                    .unwrap();
                let future = this.session.midi_clip(future).unwrap();
                assert_eq!(
                    future.recipe.as_ref().unwrap().drum_map.as_ref(),
                    Some(&map)
                );
                assert_eq!(future.notes.is_empty(), map.voices.is_empty());
                assert!(
                    future
                        .notes
                        .iter()
                        .all(|note| map.voices.values().any(|pitch| *pitch == note.pitch))
                );
            });
        }
    }

    #[gpui::test]
    fn completion_checks_source_freshness_without_changing_the_document(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            let track = this
                .session
                .add_drum_track("Kit", "auris.synth.drumkit")
                .unwrap();
            let report = measure_one(this, track, 0);
            let cancel = start_fixture(this, track);
            let original = this.project().clone();
            this.finish_drum_analysis(&cancel, Ok(report.clone()));
            assert_eq!(this.drum_analysis.report(track), Some(&report));
            assert_eq!(this.project(), &original);

            let cancel = start_fixture(this, track);
            this.session
                .set_track_instrument(track, "auris.synth.chiptune")
                .unwrap();
            let changed = this.project().clone();
            this.finish_drum_analysis(&cancel, Ok(report));
            assert!(
                this.drum_analysis.report(track).is_none(),
                "the previous instrument's measurements must disappear"
            );
            assert!(cancel.load(Ordering::Relaxed));
            assert_eq!(this.project(), &changed);
        });
    }

    #[gpui::test]
    fn composing_cancels_old_measurements_and_late_results_cannot_replace_a_new_job(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            let track = this
                .session
                .add_drum_track("Old Kit", "auris.synth.drumkit")
                .unwrap();
            let report = measure_one(this, track, 0);
            accept_fixture(this, report.clone());
            let old = start_fixture(this, track);
            let spec = SongSpec::parse(
                r#"
                form = "chorus"
                ending = "none"
                [section.chorus]
                bars = 1
                [[part]]
                name = "kick"
                role = "kick"
            "#,
            )
            .unwrap();
            this.compose_spec(&spec);
            assert!(old.load(Ordering::Relaxed));
            assert!(this.drum_analysis.report(track).is_none());
            let composed = this.project().clone();
            this.finish_drum_analysis(&old, Ok(report.clone()));
            assert!(this.drum_analysis.report(track).is_none());
            assert_eq!(this.project(), &composed);

            let next_track = this
                .project()
                .tracks
                .iter()
                .find(|track| track.kind.is_drum())
                .unwrap()
                .id;
            let next = start_fixture(this, next_track);
            this.finish_drum_analysis(&old, Ok(report));
            assert!(
                this.drum_analysis
                    .start_next(Instant::now() + state::DEBOUNCE)
                    .is_none()
            );
            assert!(!next.load(Ordering::Relaxed));
            assert!(this.drum_analysis.report(next_track).is_none());
        });
    }

    #[gpui::test]
    fn automatic_observation_keeps_each_drum_result_across_selection_and_note_edits(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            let melodic = this
                .session
                .add_instrument_track("Keys", "auris.synth.drumkit")
                .unwrap();
            let a = this.session.add_default_drum_track("A").unwrap();
            let b = this.session.add_default_drum_track("B").unwrap();
            let now = Instant::now();
            this.observe_drum_sources(now);
            assert!(this.drum_analysis.outcome(melodic).is_none());
            assert!(this.drum_analysis.start_next(now).is_none());
            let original = this.project().clone();
            for _ in 0..2 {
                let job = this
                    .drum_analysis
                    .start_next(now + state::DEBOUNCE)
                    .unwrap();
                this.select_track(melodic);
                let report = measure_one(this, job.track, 0);
                this.finish_drum_analysis(&job.cancel, Ok(report));
            }
            assert!(this.drum_analysis.report(a).is_some());
            assert!(this.drum_analysis.report(b).is_some());
            assert_eq!(this.project(), &original);
            this.session
                .generate_clip(
                    a,
                    Ticks::ZERO,
                    Ticks::QUARTER * 4,
                    ClipRecipe::new(ClipPreset::Kick, 3),
                )
                .unwrap();
            this.observe_drum_sources(now + state::DEBOUNCE * 2);
            assert!(
                this.drum_analysis
                    .start_next(now + state::DEBOUNCE * 3)
                    .is_none()
            );
            assert!(this.drum_analysis.report(a).is_some());
            assert!(this.drum_analysis.report(b).is_some());
        });
    }

    #[gpui::test]
    fn cancellation_observes_a_sound_changed_since_the_last_poll(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            let track = this.session.add_default_drum_track("Kit").unwrap();
            let old = start_fixture(this, track);
            this.session
                .set_track_instrument(track, "auris.synth.chiptune")
                .unwrap();
            this.cancel_drum_analysis(track);
            assert!(old.load(Ordering::Relaxed));
            assert!(matches!(
                this.drum_analysis.outcome(track),
                Some(Outcome::Cancelled)
            ));
            this.drum_analysis.take_finished(&old);
            let later = Instant::now() + state::DEBOUNCE;
            this.observe_drum_sources(later);
            assert!(this.drum_analysis.start_next(later).is_none());
        });
    }
}
