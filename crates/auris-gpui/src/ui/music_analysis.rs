//! Worker-based CPU recognition and explicit acceptance in a dedicated panel.

use crate::{
    app::AurisApp,
    ui::{
        context_menu::MenuCommand,
        prompt::Prompt,
        widgets::{ButtonStyle, button},
    },
};
use auris_i18n::Key;
use auris_session::prelude::*;
use auris_session::{AnalysisControl, ChordAnalysisReport, ChordState, SessionError};
use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*};

/// Selection-based entries in the application analysis menu.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnalysisCommand {
    /// Harmony from the selected note track.
    SelectedChords,
    /// Harmony across note tracks.
    AllChords,
    /// Drum measurements and mapping.
    Drums,
    /// Progress and acceptance controls.
    Results,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum MusicAnalysisOutcome {
    #[default]
    Empty,
    Stale,
    Cancelled,
    Failed(String),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum MusicAnalysisViewState {
    Empty,
    Running,
    Ready,
    Stale,
    Cancelled,
    Failed,
}

fn music_analysis_view_state(
    running: bool,
    has_report: bool,
    report_current: bool,
    outcome: &MusicAnalysisOutcome,
) -> MusicAnalysisViewState {
    if running {
        MusicAnalysisViewState::Running
    } else if has_report {
        if report_current {
            MusicAnalysisViewState::Ready
        } else {
            MusicAnalysisViewState::Stale
        }
    } else {
        match outcome {
            MusicAnalysisOutcome::Empty => MusicAnalysisViewState::Empty,
            MusicAnalysisOutcome::Stale => MusicAnalysisViewState::Stale,
            MusicAnalysisOutcome::Cancelled => MusicAnalysisViewState::Cancelled,
            MusicAnalysisOutcome::Failed(_) => MusicAnalysisViewState::Failed,
        }
    }
}

fn scored_candidate(label: &str, score_label: &str, score: f32) -> String {
    format!("{label} · {score_label} {score:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{
        CLIP_LENGTH, click, drag_to, lane_point, open, paint, press, release, with_a_clip,
    };
    use gpui::{Entity, TestAppContext, VisualTestContext};

    fn analysis_window(app: &Entity<AurisApp>, cx: &mut VisualTestContext) -> VisualTestContext {
        let handle = app.read_with(cx, |app, _| {
            app.auxiliary_windows[&crate::auxiliary_window::Surface::Analysis]
        });
        VisualTestContext::from_window(handle.into(), cx)
    }

    #[test]
    fn analysis_candidate_labels_name_the_unit_and_score() {
        assert_eq!(
            scored_candidate("Cmaj7", "score", 0.876),
            "Cmaj7 · score 0.88"
        );
    }

    #[test]
    fn music_analysis_state_distinguishes_every_terminal_outcome() {
        let state = |running, report, current, outcome| {
            music_analysis_view_state(running, report, current, outcome)
        };
        assert_eq!(
            state(false, false, true, &MusicAnalysisOutcome::Empty),
            MusicAnalysisViewState::Empty
        );
        assert_eq!(
            state(true, false, true, &MusicAnalysisOutcome::Empty),
            MusicAnalysisViewState::Running
        );
        assert_eq!(
            state(false, true, true, &MusicAnalysisOutcome::Empty),
            MusicAnalysisViewState::Ready
        );
        assert_eq!(
            state(false, true, false, &MusicAnalysisOutcome::Empty),
            MusicAnalysisViewState::Stale
        );
        assert_eq!(
            state(false, false, true, &MusicAnalysisOutcome::Stale),
            MusicAnalysisViewState::Stale
        );
        assert_eq!(
            state(false, false, true, &MusicAnalysisOutcome::Cancelled),
            MusicAnalysisViewState::Cancelled
        );
        assert_eq!(
            state(
                false,
                false,
                true,
                &MusicAnalysisOutcome::Failed("decoder stopped".into()),
            ),
            MusicAnalysisViewState::Failed
        );
    }

    #[gpui::test]
    fn narrow_analysis_keeps_state_primary_action_and_close_reachable(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        cx.dispatch_action(crate::actions::OpenAnalysisResults);
        paint(&app, cx);
        let mut analysis = analysis_window(&app, cx);
        analysis.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(240.0)));
        analysis.run_until_parked();

        let panel = analysis
            .debug_bounds("analysis-panel")
            .expect("the analysis panel is visible");
        for selector in ["analysis-state", "music-all", "analysis-close"] {
            let bounds = analysis
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("`{selector}` is visible"));
            assert!(bounds.size.width > gpui::px(0.0));
            assert!(bounds.left() >= panel.left());
            assert!(bounds.right() <= panel.right());
            assert!(bounds.top() >= panel.top());
            assert!(bounds.bottom() <= panel.bottom());
        }
    }

    fn written_triad(app: &mut AurisApp) -> ClipId {
        app.panels = crate::dock::PanelLayout::default();
        let track = app
            .session
            .add_default_instrument_track("Analysis fixture")
            .unwrap();
        let clip = app
            .session
            .add_midi_clip(track, "C major", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        for pitch in [60, 64, 67] {
            app.session
                .add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        }
        app.selected_track = None;
        clip
    }

    #[gpui::test]
    fn analysis_panel_reports_without_editing_then_accepts_with_undo(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let before = app.update(cx, |this, _| {
            written_triad(this);
            assert!(!this.analysis_panel);
            this.project().clone()
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("music-all").is_none());
        cx.dispatch_action(crate::actions::AnalyzeAllChords);
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(this.music_analysis.report.is_some());
            assert_eq!(this.project(), &before);
        });
        paint(&app, cx);
        let mut analysis = analysis_window(&app, cx);
        click("music-apply", &mut analysis);
        app.update(cx, |this, _| {
            assert!(this.music_analysis.report.is_none());
            assert_eq!(
                this.session
                    .harmony()
                    .chord_at(Ticks::ZERO)
                    .unwrap()
                    .to_string(),
                "C"
            );
            this.session.undo();
            assert_eq!(this.project(), &before);
        });
        paint(&app, cx);
        click("analysis-close", &mut analysis);
        app.read_with(cx, |this, _| assert!(!this.analysis_panel));
    }

    #[gpui::test]
    fn analysis_presentation_tracks_stale_failed_and_cancelled_work(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            written_triad(this);
            this.begin_chord_analysis(None, cx);
        });
        cx.run_until_parked();
        app.update(cx, |this, cx| {
            assert_eq!(this.music_analysis_state(), MusicAnalysisViewState::Ready);

            this.session
                .add_default_instrument_track("Newer document state")
                .unwrap();
            assert_eq!(this.music_analysis_state(), MusicAnalysisViewState::Stale);

            this.begin_chord_analysis(Some(TrackId(u64::MAX)), cx);
            assert_eq!(this.music_analysis_state(), MusicAnalysisViewState::Failed);
            assert!(matches!(
                &this.music_analysis.outcome,
                MusicAnalysisOutcome::Failed(detail) if !detail.is_empty()
            ));

            this.begin_chord_analysis(None, cx);
            this.run_menu_command(MenuCommand::CancelMusicAnalysis, cx);
            assert_eq!(
                this.music_analysis_state(),
                MusicAnalysisViewState::Cancelled
            );
        });
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.music_analysis_state(),
                MusicAnalysisViewState::Cancelled,
                "the cancelled worker cannot overwrite its terminal state"
            );
        });
    }

    #[gpui::test]
    fn escape_cancels_a_clip_drag_before_closing_analysis_results(cx: &mut TestAppContext) {
        let (app, cx, track, clip) = with_a_clip(cx);
        let (before, selection) = app.update(cx, |this, _| {
            this.select_track(track);
            this.select_clip(Some(clip));
            (
                this.project().clone(),
                (
                    this.selected_track,
                    this.selected_clip,
                    this.selected_clips.clone(),
                    this.selected_notes.clone(),
                ),
            )
        });
        cx.dispatch_action(crate::actions::OpenAnalysisResults);
        paint(&app, cx);
        let from = lane_point(&app, cx, track, Ticks(CLIP_LENGTH.0 / 2));
        let middle = lane_point(&app, cx, track, CLIP_LENGTH);
        let to = lane_point(&app, cx, track, Ticks(CLIP_LENGTH.0 * 3 / 2));
        press(cx, from);
        drag_to(cx, middle);
        drag_to(cx, to);
        app.read_with(cx, |this, _| {
            assert!(this.analysis_panel);
            assert!(this.dragging(), "the gesture is still held");
            assert_eq!(this.session.midi_clip(clip).unwrap().start, CLIP_LENGTH);
        });

        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| {
            assert!(
                !this.dragging(),
                "Escape must cancel the active gesture first"
            );
            assert_eq!(this.project(), &before);
            assert_eq!(
                (
                    this.selected_track,
                    this.selected_clip,
                    this.selected_clips.clone(),
                    this.selected_notes.clone(),
                ),
                selection
            );
            assert!(
                this.analysis_panel,
                "the modeless panel stays open after cancellation"
            );
        });
        release(cx, to);
        app.update(cx, |this, _| {
            assert_eq!(
                this.project(),
                &before,
                "release must not commit the abandoned drag"
            );
            this.session
                .rename_track(track, "After cancellation")
                .unwrap();
            assert_ne!(this.project(), &before);
        });
        cx.dispatch_action(crate::actions::Undo);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.project(),
                &before,
                "a later edit has its own undo step"
            );
        });
        let mut analysis = analysis_window(&app, cx);
        analysis.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| assert!(!this.analysis_panel));
    }

    #[gpui::test]
    fn stale_cancelled_and_replaced_documents_do_not_receive_worker_results(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            let clip = written_triad(this);
            this.begin_chord_analysis(None, cx);
            this.session
                .add_note(clip, Note::new(61, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        });
        cx.run_until_parked();
        app.update(cx, |this, cx| {
            assert!(this.music_analysis.report.is_none());
            this.begin_chord_analysis(None, cx);
            this.run_menu_command(MenuCommand::CancelMusicAnalysis, cx);
        });
        cx.run_until_parked();
        app.update(cx, |this, cx| {
            assert!(this.music_analysis.report.is_none());
            this.begin_chord_analysis(None, cx);
            this.new_project();
        });
        cx.run_until_parked();
        app.read_with(cx, |this, _| assert!(this.music_analysis.report.is_none()));
    }
}

/// One active worker and its last result, owned by the frontend.
#[derive(Default)]
pub(crate) struct MusicAnalysisState {
    generation: u64,
    control: Option<AnalysisControl>,
    /// Current draft, cleared when the document is replaced or the draft is accepted.
    pub(crate) report: Option<ChordAnalysisReport>,
    outcome: MusicAnalysisOutcome,
}

impl MusicAnalysisState {
    /// Cancels pending work and invalidates any completion already queued for the UI.
    pub(crate) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(control) = self.control.take() {
            control.cancel();
        }
    }

    /// Discards presentation state when a new document or analysis replaces it.
    pub(crate) fn clear(&mut self) {
        self.cancel();
        self.report = None;
        self.outcome = MusicAnalysisOutcome::Empty;
    }

    /// Records an explicit user cancellation after invalidating the active worker.
    pub(crate) fn cancel_requested(&mut self) {
        self.cancel();
        self.report = None;
        self.outcome = MusicAnalysisOutcome::Cancelled;
    }

    fn fail(&mut self, detail: String) {
        self.cancel();
        self.report = None;
        self.outcome = MusicAnalysisOutcome::Failed(detail);
    }

    fn stale(&mut self) {
        self.cancel();
        self.report = None;
        self.outcome = MusicAnalysisOutcome::Stale;
    }
}

impl AurisApp {
    /// Dispatches the analysis menu against the current selection.
    pub(crate) fn open_analysis_command(
        &mut self,
        command: AnalysisCommand,
        cx: &mut Context<Self>,
    ) {
        use AnalysisCommand::*;
        let track = self.selected_track.and_then(|id| self.project().track(id));
        let action = match command {
            SelectedChords => track
                .filter(|t| !t.kind.is_drum() && t.kind.note_clips().is_some())
                .map(|t| MenuCommand::AnalyzeChords(Some(t.id))),
            AllChords => Some(MenuCommand::AnalyzeChords(None)),
            Drums if track.is_some_and(|t| t.kind.is_drum()) => None,
            Results => None,
            _ => {
                self.set_status(self.t(Key::AnalysisSelectSource));
                cx.notify();
                return;
            }
        };
        if !matches!(command, Drums | Results) && action.is_none() {
            self.set_status(self.t(Key::AnalysisSelectSource));
            cx.notify();
            return;
        }
        self.analysis_panel = true;
        self.raise_auxiliary = Some(crate::auxiliary_window::Surface::Analysis);
        if let Some(action) = action {
            self.run_menu_command(action, cx);
        }
        cx.notify();
    }

    /// A scrollable analysis workspace, opened explicitly from the menu bar.
    pub(crate) fn render_analysis_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.analysis_panel {
            return None;
        }
        let mut rows = self.music_analysis_rows(cx);
        if let Some(track) = self.selected_track {
            rows.extend(self.drum_analysis_rows(track, cx));
        }
        Some(
            div()
                .id("analysis-panel")
                .debug_selector(|| "analysis-panel".to_owned())
                .size_full()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .bg(self.theme.surface)
                .occlude()
                .border_1()
                .border_color(self.theme.border)
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(self.t(Key::AnalysisTitle)),
                        )
                        .child(button(
                            "analysis-close",
                            self.t(Key::Close),
                            ButtonStyle::Normal,
                            false,
                            self.theme.accent,
                            &self.theme,
                            cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
                                this.analysis_panel = false;
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .id("analysis-results-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    fn fail_music_analysis(&mut self, detail: impl Into<String>) {
        let detail = detail.into();
        self.music_analysis.fail(detail.clone());
        self.set_failed_status(detail);
    }

    /// Schedules analysis of all pitched tracks or the selected note track.
    pub(crate) fn begin_chord_analysis(&mut self, track: Option<TrackId>, cx: &mut Context<Self>) {
        let tracks: Vec<_> = track.into_iter().collect();
        let (from, to) = match track.and_then(|id| self.project().track(id)) {
            Some(track) => {
                let clips: Vec<_> = track.kind.note_clips().into_iter().flatten().collect();
                (
                    clips.iter().map(|c| c.start).min().unwrap_or(Ticks::ZERO),
                    clips
                        .iter()
                        .map(|c| c.sounding_end())
                        .max()
                        .unwrap_or(Ticks::ZERO),
                )
            }
            None => (Ticks::ZERO, self.project().end_tick()),
        };
        match self
            .session
            .chord_analysis_job(&tracks, from, to, Default::default())
        {
            Ok(job) => self.start_music_worker(move |control| job.run(control), cx),
            Err(e) => self.fail_music_analysis(e.to_string()),
        }
    }

    /// Follows an audio clip as one voice on a worker, then writes what it sang onto a new
    /// instrument track under the clip, as one undo step.
    ///
    /// Applied as soon as it is ready rather than held as a draft in the analysis panel: what it
    /// adds is a track of its own beside the audio, which Undo removes, and there is no choice
    /// among readings to make first.
    pub(crate) fn begin_hum_transcription(&mut self, clip: ClipId, cx: &mut Context<Self>) {
        let job = match self.session.audio_analysis_job(clip) {
            Ok(job) => job,
            Err(error) => {
                self.set_failed_status(error.to_string());
                return;
            }
        };
        let name = self
            .audio_clip(clip)
            .map(|found| found.name.clone())
            .unwrap_or_default();
        let name = self.t(Key::HumNotesTrackName).replace("{name}", &name);
        self.set_status(self.t(Key::HumToNotesRunning));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { job.run_hum(&AnalysisControl::default()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let landed = result.and_then(|report| {
                    let count = report.notes.len();
                    this.session
                        .import_hum_transcription(&report, &name)
                        .map(|(track, _)| (track, count))
                });
                match landed {
                    Ok((track, count)) => {
                        this.selected_track = Some(track);
                        this.set_status(
                            this.t(Key::HumToNotesDone)
                                .replace("{count}", &count.to_string()),
                        );
                    }
                    Err(error) => this.set_failed_status(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_music_worker(
        &mut self,
        work: impl FnOnce(&AnalysisControl) -> Result<ChordAnalysisReport, SessionError>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        self.music_analysis.clear();
        let generation = self.music_analysis.generation;
        let control = AnalysisControl::default();
        self.music_analysis.control = Some(control.clone());
        self.analysis_panel = true;
        self.set_status(self.t(Key::AnalysisRunning));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&control) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.music_analysis.generation != generation {
                    return;
                }
                this.music_analysis.control = None;
                match result {
                    Ok(report) if this.music_report_current(&report) => {
                        this.music_analysis.report = Some(report);
                        this.music_analysis.outcome = MusicAnalysisOutcome::Empty;
                        this.set_status(this.t(Key::AnalysisReady));
                    }
                    Ok(_) => {
                        this.music_analysis.stale();
                        this.set_status(this.t(Key::AnalysisStale));
                    }
                    Err(e) => this.fail_music_analysis(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let keep = this
                    .update(cx, |this, cx| {
                        let active = this.music_analysis.generation == generation
                            && this.music_analysis.control.is_some();
                        if active {
                            cx.notify();
                        }
                        active
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn music_report_current(&self, report: &ChordAnalysisReport) -> bool {
        self.session.chord_analysis_is_current(report)
    }

    fn music_analysis_state(&self) -> MusicAnalysisViewState {
        let report_current = self
            .music_analysis
            .report
            .as_ref()
            .is_none_or(|report| self.music_report_current(report));
        music_analysis_view_state(
            self.music_analysis.control.is_some(),
            self.music_analysis.report.is_some(),
            report_current,
            &self.music_analysis.outcome,
        )
    }

    /// Applies the user's chosen report action as an undoable session command.
    pub(crate) fn accept_music_analysis(&mut self, choice: Option<(usize, usize)>) {
        let Some(report) = self.music_analysis.report.clone() else {
            return;
        };
        let outcome = match choice {
            Some((segment, candidate)) => self
                .session
                .apply_chord_candidate(&report, segment, candidate),
            None => self.session.apply_chord_analysis(&report),
        }
        .map(|_| ());
        match outcome {
            Ok(()) => {
                self.music_analysis.report = None;
                self.music_analysis.outcome = MusicAnalysisOutcome::Empty;
                self.set_status(self.t(Key::AnalysisApplyChords));
            }
            Err(e) => self.fail_music_analysis(e.to_string()),
        }
    }

    /// Opens the report's bounded textual preview.
    pub(crate) fn view_music_analysis(&mut self) {
        let Some(report) = &self.music_analysis.report else {
            return;
        };
        let mut lines: Vec<SharedString> = vec![self.t(Key::AnalysisScores).into()];
        let reading = |r: &auris_session::ChordReading| {
            if r.state == ChordState::NoChord {
                return self.t(Key::AnalysisNoChord).to_string();
            }
            let alternatives = r
                .candidates
                .iter()
                .map(|c| format!("{} ({:.2})", c.symbol, c.score))
                .collect::<Vec<_>>()
                .join(" / ");
            format!(
                "{}{}",
                if r.state == ChordState::Unknown {
                    format!("{}: ", self.t(Key::AnalysisUnknown))
                } else {
                    String::new()
                },
                alternatives
            )
        };
        lines.push(self.t(Key::AnalysisBeatUnits).into());
        lines.extend(report.segments.iter().take(256).map(|s| {
            format!(
                "{:.2}–{:.2}: {}",
                s.start.raw() as f64 / 960.0,
                s.end.raw() as f64 / 960.0,
                reading(&s.reading)
            )
            .into()
        }));
        if report.segments.len() > 256 {
            lines.push(self.t(Key::AnalysisPreviewLimit).into());
        }
        self.open_prompt(Prompt::notice(self.t(Key::AnalysisTitle), lines));
    }

    /// Renders worker progress and the last report's acceptance controls.
    pub(crate) fn music_analysis_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows = Vec::new();
        let action_button = |id: &'static str, label: String, action: MenuCommand| {
            button(
                id,
                label,
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.run_menu_command(action.clone(), cx);
                    cx.notify();
                }),
            )
            .into_any_element()
        };
        let view_state = self.music_analysis_state();
        let state = match view_state {
            MusicAnalysisViewState::Running => format!(
                "{} {:.0}%",
                self.t(Key::AnalysisMusicRunning),
                self.music_analysis
                    .control
                    .as_ref()
                    .map_or(0.0, |control| control.progress())
                    * 100.0
            ),
            MusicAnalysisViewState::Ready => self.t(Key::AnalysisMusicReady).to_string(),
            MusicAnalysisViewState::Stale => self.t(Key::AnalysisMusicStale).to_string(),
            MusicAnalysisViewState::Cancelled => self.t(Key::AnalysisMusicCancelled).to_string(),
            MusicAnalysisViewState::Failed => self.t(Key::AnalysisMusicFailed).to_string(),
            MusicAnalysisViewState::Empty => self.t(Key::AnalysisMusicEmpty).to_string(),
        };
        rows.push(
            div()
                .debug_selector(|| "analysis-state".to_owned())
                .text_xs()
                .text_color(theme.text_muted)
                .whitespace_normal()
                .child(state)
                .into_any_element(),
        );
        if view_state == MusicAnalysisViewState::Failed
            && let MusicAnalysisOutcome::Failed(detail) = &self.music_analysis.outcome
        {
            rows.push(
                div()
                    .debug_selector(|| "analysis-state-detail".to_owned())
                    .text_xs()
                    .text_color(theme.danger)
                    .whitespace_normal()
                    .child(detail.clone())
                    .into_any_element(),
            );
        }
        rows.push(action_button(
            "music-all",
            self.t(Key::MenuAnalyzeAllChords).to_string(),
            MenuCommand::AnalyzeChords(None),
        ));
        if self.music_analysis.control.is_some() {
            rows.push(action_button(
                "music-cancel",
                self.t(Key::AnalysisCancel).to_string(),
                MenuCommand::CancelMusicAnalysis,
            ));
        }
        let Some(report) = &self.music_analysis.report else {
            return rows;
        };
        rows.push(
            div()
                .debug_selector(|| "analysis-score-help".to_owned())
                .text_xs()
                .text_color(theme.text_muted)
                .whitespace_normal()
                .child(self.t(Key::AnalysisScores))
                .into_any_element(),
        );
        rows.push(action_button(
            "music-view",
            self.t(Key::AnalysisView).to_string(),
            MenuCommand::ViewMusicAnalysis,
        ));
        // Validation is performed on completion and application, not while painting every frame.
        rows.push(action_button(
            "music-apply",
            self.t(Key::AnalysisApplyChords).to_string(),
            MenuCommand::ApplyMusicChords,
        ));
        rows.push(
            div()
                .text_xs()
                .child(self.t(Key::AnalysisBeatUnits))
                .into_any_element(),
        );
        for (i, s) in report.segments.iter().take(256).enumerate() {
            rows.push(
                div()
                    .text_xs()
                    .child(format!(
                        "{:.2}–{:.2}{}",
                        s.start.raw() as f64 / 960.0,
                        s.end.raw() as f64 / 960.0,
                        if s.reading.state == ChordState::Unknown {
                            format!(" · {}", self.t(Key::AnalysisUnknown))
                        } else {
                            String::new()
                        }
                    ))
                    .into_any_element(),
            );
            for (j, c) in s.reading.candidates.iter().enumerate() {
                rows.push(
                    button(
                        ("music-candidate", i * 4 + j),
                        scored_candidate(&c.symbol, self.t(Key::AnalysisScoreLabel), c.score),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            this.run_menu_command(
                                MenuCommand::ApplyMusicCandidate {
                                    segment: i,
                                    candidate: j,
                                },
                                cx,
                            );
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
                );
            }
        }
        if report.segments.len() > 256 {
            rows.push(
                div()
                    .text_xs()
                    .child(self.t(Key::AnalysisPreviewLimit))
                    .into_any_element(),
            );
        }
        rows
    }
}
