//! Acoustic kit measurements and their explicit application to a drum track.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

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
    /// Starts an isolated process after snapshotting this instrument on the owning thread.
    pub(crate) fn begin_drum_analysis(&mut self, track: TrackId, cx: &mut Context<Self>) {
        let request = match self.session.drum_probe_request(track, &Default::default()) {
            Ok(request) => request,
            Err(error) => {
                self.set_failed_status(error.to_string());
                return;
            }
        };
        self.cancel_drum_analysis();
        self.drum_analysis = None;
        let cancel = Arc::new(AtomicBool::new(false));
        self.drum_analysis_cancel = Some(Arc::clone(&cancel));
        self.set_status(self.t(Key::DrumAnalysisRunning));
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
        if !self
            .drum_analysis_cancel
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, cancel))
        {
            return;
        }
        self.drum_analysis_cancel = None;
        self.drum_analysis = None;
        match report {
            Ok(report) => match self
                .session
                .drum_probe_request(report.track, &report.options)
            {
                Ok(current)
                    if current.track == report.track
                        && current.source_fingerprint == report.source_fingerprint
                        && current.sample_rate == report.sample_rate
                        && current.bpm == report.bpm =>
                {
                    self.drum_analysis = Some(report);
                    self.set_status(self.t(Key::DrumAnalysisReady));
                }
                Ok(_) => self.set_failed_status(self.t(Key::DrumAnalysisObsolete)),
                Err(error) => self.set_failed_status(error.to_string()),
            },
            Err(error) => self.set_failed_status(error.to_string()),
        }
    }

    /// Cancels a pending process without changing the last accepted map.
    pub(crate) fn cancel_drum_analysis(&mut self) {
        if let Some(cancel) = self.drum_analysis_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
            self.set_status(self.t(Key::MenuCancelDrumAnalysis));
        }
    }

    /// Applies the measured proposal only after the user chooses its action.
    pub(crate) fn apply_measured_drums(&mut self, track: TrackId, remap_generated: bool) {
        let Some(report) = self
            .drum_analysis
            .as_ref()
            .filter(|report| report.track == track)
            .cloned()
        else {
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
        let (label, action) = if self.drum_analysis_cancel.is_some() {
            (Key::MenuCancelDrumAnalysis, MenuCommand::CancelDrumAnalysis)
        } else {
            (Key::MenuAnalyzeDrums, MenuCommand::AnalyzeDrums(track))
        };
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
        let Some(report) = self
            .drum_analysis
            .as_ref()
            .filter(|report| report.track == track)
        else {
            return rows;
        };
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
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::{DrumKitAnalysis, DrumScanOptions};
    use gpui::TestAppContext;

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

    #[gpui::test]
    fn future_only_button_accepts_empty_and_incomplete_maps_without_rewriting_clips(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
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
                this.drum_analysis = Some(report);
                (track, clip, original, map)
            });
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
            let cancel = Arc::new(AtomicBool::new(false));
            this.drum_analysis_cancel = Some(Arc::clone(&cancel));
            let original = this.project().clone();
            this.finish_drum_analysis(&cancel, Ok(report.clone()));
            assert_eq!(this.drum_analysis.as_ref(), Some(&report));
            assert_eq!(this.project(), &original);

            this.drum_analysis_cancel = Some(Arc::clone(&cancel));
            this.session
                .set_track_instrument(track, "auris.synth.chiptune")
                .unwrap();
            let changed = this.project().clone();
            this.finish_drum_analysis(&cancel, Ok(report));
            assert!(
                this.drum_analysis.is_none(),
                "the previous instrument's measurements must disappear"
            );
            assert!(this.drum_analysis_cancel.is_none());
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
            let old = Arc::new(AtomicBool::new(false));
            this.drum_analysis_cancel = Some(Arc::clone(&old));
            this.drum_analysis = Some(report.clone());
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
            assert!(this.drum_analysis_cancel.is_none());
            assert!(this.drum_analysis.is_none());
            let composed = this.project().clone();
            this.finish_drum_analysis(&old, Ok(report.clone()));
            assert!(this.drum_analysis.is_none());
            assert_eq!(this.project(), &composed);

            let next = Arc::new(AtomicBool::new(false));
            this.drum_analysis_cancel = Some(Arc::clone(&next));
            this.finish_drum_analysis(&old, Ok(report));
            assert!(
                this.drum_analysis_cancel
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &next))
            );
            assert!(!next.load(Ordering::Relaxed));
            assert!(this.drum_analysis.is_none());
        });
    }
}
