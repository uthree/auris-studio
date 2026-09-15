//! Audio objective controls and comparisons of captured renders.

use crate::theme::{Metrics, Theme};
use crate::ui::widgets::{ButtonState, ButtonStyle, button, button_enabled, divider};
use gpui::{
    AnyElement, Context, ElementId, IntoElement, SharedString, Window, canvas, div, prelude::*, px,
    relative,
};

use super::*;

#[derive(Copy, Clone, PartialEq, Eq)]
enum LastControl {
    Close,
    ScopeArrangement,
    PreviewSource,
    PreviewBest,
    PreviewStop,
    Start,
    Cancel,
    Apply,
}

impl AurisApp {
    fn reference_focus_region(
        &self,
        child: AnyElement,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (focus, bounds) = self.modal_focus.reference_match_reveal_target(index, cx);
        div()
            .id(("reference-focus-region", index))
            .debug_selector(move || format!("reference-focus-region-{index}"))
            .relative()
            .flex()
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .track_focus(&focus)
            .child(child)
            .child(
                canvas(
                    move |measured, _, _| bounds.set(Some(measured)),
                    |_, _, _, _| (),
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    fn reference_focus_regions(
        &self,
        rows: Vec<AnyElement>,
        cursor: &mut usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        rows.into_iter()
            .map(|child| {
                let index = *cursor;
                *cursor += 1;
                self.reference_focus_region(child, index, cx)
            })
            .collect()
    }

    /// Draws a modal comparison workspace without changing the current arrangement.
    pub(crate) fn render_reference_match(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = &self.reference_match;
        if !state.open {
            return None;
        }
        let theme = &self.theme;
        let editable = !state.busy();
        let stale = state
            .comparison
            .as_ref()
            .is_some_and(|result| !self.match_snapshot_is_current(&result.snapshot));
        let can_preview = state.comparison.is_some() && !stale;
        let can_start = editable && state.input_problem().is_none();
        let can_apply = editable && can_preview;
        let last_control = if can_apply {
            LastControl::Apply
        } else if state.running.is_some() {
            LastControl::Cancel
        } else if can_start {
            LastControl::Start
        } else if state.preview.is_some() {
            LastControl::PreviewStop
        } else if can_preview {
            LastControl::PreviewBest
        } else if state.objective.needs_reference() && state.source.is_some() {
            LastControl::PreviewSource
        } else if editable {
            LastControl::ScopeArrangement
        } else {
            LastControl::Close
        };
        let viewport = window.viewport_size();
        let width = (viewport.width - px(32.0)).max(px(0.0)).min(px(920.0));
        if self.modal_focus.reference_match().is_focused(window) {
            self.modal_focus.reset_reference_match_scroll();
        }
        let scroll = self.modal_focus.reference_match_scroll().clone();
        let mut focus_region = 0usize;
        let target = self.audio_match_target(cx);
        let target = self.reference_focus_region(target, focus_region, cx);
        focus_region += 1;
        let source = if state.objective.needs_reference() {
            let source_row = div()
                .flex()
                .flex_wrap()
                .gap_2()
                .items_center()
                .child(button_enabled(
                    "reference-match-file",
                    self.t(Key::ReferenceMatchChoose),
                    ButtonStyle::Normal,
                    ButtonState::available(false, editable),
                    theme.accent,
                    theme,
                    cx.listener(|this, _, _, cx| this.choose_reference_audio(cx)),
                ))
                .when_some(state.source.as_ref(), |this, source| {
                    this.child(
                        div().min_w_0().text_xs().child(
                            source
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        ),
                    )
                });
            let source = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(source_row)
                .when_some(state.source.as_ref(), |this, source| {
                    this.child(div().text_xs().text_color(theme.text_muted).child(format!(
                        "{}: {:.1} s",
                        self.t(Key::ReferenceMatchFileDuration),
                        source.audio.duration_seconds()
                    )))
                })
                .into_any_element();
            let source = self.reference_focus_region(source, focus_region, cx);
            focus_region += 1;
            Some(source)
        } else {
            None
        };
        let mut body = div()
            .id("reference-match-scroll")
            .debug_selector(|| "reference-match-scroll".to_string())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .flex()
            .flex_col()
            .gap_3()
            .pr_2()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::ReferenceMatchHint)),
            )
            .child(target)
            .children(source);
        let mut settings = vec![
            reference_row(self, Key::ReferenceMatchProjectStart)
                .child(choice(
                    self,
                    cx,
                    "reference-project-less",
                    "−",
                    editable && state.settings.project_start_seconds > 0.0,
                    false,
                    |state| {
                        state.settings.project_start_seconds =
                            (state.settings.project_start_seconds - 1.0).max(0.0)
                    },
                ))
                .child(number(state.settings.project_start_seconds))
                .child(choice(
                    self,
                    cx,
                    "reference-project-more",
                    "+",
                    editable,
                    false,
                    |state| state.settings.project_start_seconds += 1.0,
                ))
                .child(button_enabled(
                    "reference-project-playhead",
                    self.t(Key::ReferenceMatchPlayhead),
                    ButtonStyle::Ghost,
                    ButtonState::available(false, editable),
                    theme.accent,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        if this.reference_match.busy() {
                            return;
                        }
                        let seconds = this
                            .project()
                            .tempo_map
                            .ticks_to_seconds(this.playhead_ticks())
                            .0;
                        this.change_reference_settings(|state| {
                            state.settings.project_start_seconds = seconds
                        });
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        ];
        if state.objective.needs_reference() {
            settings.push(
                reference_row(self, Key::ReferenceMatchReferenceStart)
                    .child(choice(
                        self,
                        cx,
                        "reference-source-less",
                        "−",
                        editable && state.reference_start > 0.0,
                        false,
                        |state| state.reference_start = (state.reference_start - 1.0).max(0.0),
                    ))
                    .child(number(state.reference_start))
                    .child(choice(
                        self,
                        cx,
                        "reference-source-more",
                        "+",
                        editable,
                        false,
                        |state| state.reference_start += 1.0,
                    ))
                    .into_any_element(),
            );
        }
        settings.push(
            reference_row(self, Key::ReferenceMatchDuration)
                .children([1usize, 2, 5, 10, 12, 20, 30].into_iter().map(|seconds| {
                    choice(
                        self,
                        cx,
                        ("reference-duration", seconds),
                        seconds.to_string(),
                        editable,
                        state.settings.duration_seconds == seconds as f64,
                        move |state| state.settings.duration_seconds = seconds as f64,
                    )
                }))
                .into_any_element(),
        );
        settings.push(
            reference_row(self, Key::SongSearchAttempts)
                .children(
                    [8usize, 16, 32, 64, 128, 256, 512]
                        .into_iter()
                        .map(|attempts| {
                            choice(
                                self,
                                cx,
                                ("reference-attempts", attempts),
                                attempts.to_string(),
                                editable,
                                state.settings.attempts == attempts,
                                move |state| state.settings.attempts = attempts,
                            )
                        }),
                )
                .into_any_element(),
        );
        settings.push(
            reference_row(self, Key::SongSearchSeed)
                .child(choice(
                    self,
                    cx,
                    "reference-seed-less",
                    "−",
                    editable && state.settings.seed > 0,
                    false,
                    |state| state.settings.seed = state.settings.seed.saturating_sub(1),
                ))
                .child(
                    div()
                        .min_w(px(44.0))
                        .text_center()
                        .child(state.settings.seed.to_string()),
                )
                .child(choice(
                    self,
                    cx,
                    "reference-seed-more",
                    "+",
                    editable,
                    false,
                    |state| state.settings.seed = state.settings.seed.saturating_add(1),
                ))
                .into_any_element(),
        );
        let settings = self.reference_focus_regions(settings, &mut focus_region, cx);
        let scope = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(choice(
                self,
                cx,
                "reference-scope-mix",
                self.t(Key::ReferenceMatchMix),
                editable,
                state.settings.mix,
                |state| state.settings.mix = !state.settings.mix,
            ))
            .child(choice(
                self,
                cx,
                "reference-scope-performance",
                self.t(Key::ReferenceMatchPerformance),
                editable,
                state.settings.performance,
                |state| state.settings.performance = !state.settings.performance,
            ))
            .child(choice(
                self,
                cx,
                "reference-scope-generation-seeds",
                self.t(Key::ReferenceMatchGenerationSeeds),
                editable,
                state.settings.generation_seeds,
                |state| state.settings.generation_seeds = !state.settings.generation_seeds,
            ))
            .child(choice(
                self,
                cx,
                "reference-scope-instruments",
                self.t(Key::ReferenceMatchInstruments),
                editable,
                state.settings.instruments,
                |state| state.settings.instruments = !state.settings.instruments,
            ))
            .child(
                choice(
                    self,
                    cx,
                    "reference-scope-arrangement",
                    self.t(Key::ReferenceMatchArrangement),
                    editable,
                    state.settings.arrangement,
                    |state| state.settings.arrangement = !state.settings.arrangement,
                )
                .when(last_control == LastControl::ScopeArrangement, |this| {
                    this.track_focus(self.modal_focus.reference_match_last())
                }),
            )
            .into_any_element();
        let scope = self.reference_focus_region(scope, focus_region, cx);
        focus_region += 1;
        body = body
            .child(
                div()
                    .grid()
                    .grid_cols(if width >= px(760.0) { 2 } else { 1 })
                    .gap_3()
                    .children(settings),
            )
            .child(scope)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::ReferenceMatchScope)),
            );
        let preview_controls = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(
                [
                    (
                        0usize,
                        "reference-preview-source",
                        Key::ReferenceMatchPreviewReference,
                        state.source.is_some(),
                    ),
                    (
                        1,
                        "reference-preview-before",
                        Key::ReferenceMatchPreviewBefore,
                        can_preview,
                    ),
                    (
                        2,
                        "reference-preview-best",
                        Key::ReferenceMatchPreviewBest,
                        can_preview,
                    ),
                ]
                .into_iter()
                .filter(|(selection, _, _, _)| *selection != 0 || state.objective.needs_reference())
                .map(|(selection, id, label, enabled)| {
                    let active = state
                        .preview
                        .as_ref()
                        .is_some_and(|preview| preview.selection == selection);
                    let is_last = (selection == 0 && last_control == LastControl::PreviewSource)
                        || (selection == 2 && last_control == LastControl::PreviewBest);
                    button_enabled(
                        id,
                        self.t(label),
                        ButtonStyle::Normal,
                        ButtonState::available(active, enabled),
                        theme.accent,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.preview_reference_match(selection, cx);
                            }
                        }),
                    )
                    .when(is_last, |this| {
                        this.track_focus(self.modal_focus.reference_match_last())
                    })
                    .into_any_element()
                }),
            )
            .child(
                button_enabled(
                    "reference-preview-stop",
                    self.t(Key::ReferenceMatchStop),
                    ButtonStyle::Ghost,
                    ButtonState::available(false, state.preview.is_some()),
                    theme.accent,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        if this.reference_match.preview.is_some() {
                            this.stop_reference_preview();
                            cx.notify();
                        }
                    }),
                )
                .when(last_control == LastControl::PreviewStop, |this| {
                    this.track_focus(self.modal_focus.reference_match_last())
                }),
            )
            .into_any_element();
        let preview_controls = self.reference_focus_region(preview_controls, focus_region, cx);
        body = body
            .child(divider(theme))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::ReferenceMatchPreviewHint)),
            )
            .child(preview_controls);
        if let Some(comparison) = &state.comparison {
            let report = &comparison.report;
            body = body
                .child(divider(theme))
                .child(div().text_sm().child(self.t(
                    if comparison.snapshot.objective.uses_clap() {
                        Key::AudioMatchSimilarity
                    } else {
                        Key::ReferenceMatchDistance
                    },
                )));
            for (name, label) in [
                ("clap_cosine_similarity", Key::AudioMatchCosine),
                ("reference_distance", Key::ReferenceMatchDistance),
                ("reference_spectrum_distance", Key::ReferenceMatchSpectrum),
                ("reference_dynamics_distance", Key::ReferenceMatchDynamics),
                ("reference_stereo_distance", Key::ReferenceMatchStereo),
                ("reference_rhythm_distance", Key::ReferenceMatchRhythm),
            ] {
                let before = report
                    .baseline
                    .metrics
                    .iter()
                    .find(|metric| metric.name == name)
                    .map(|metric| metric.value);
                let best = report
                    .best
                    .metrics
                    .iter()
                    .find(|metric| metric.name == name)
                    .map(|metric| metric.value);
                if let (Some(before), Some(best)) = (before, best) {
                    let precision = if comparison.snapshot.objective.uses_clap() {
                        6
                    } else {
                        4
                    };
                    body = body.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .text_xs()
                            .child(div().min_w(px(180.0)).child(self.t(label)))
                            .child(format!(
                                "{}: {before:.precision$}",
                                self.t(Key::ReferenceMatchBefore),
                            ))
                            .child(format!(
                                "{}: {best:.precision$}",
                                self.t(Key::ReferenceMatchBest)
                            )),
                    );
                }
            }
            body = body
                .child(div().text_xs().text_color(theme.text_muted).child(self.t(
                    if comparison.snapshot.objective.uses_clap() {
                        Key::AudioMatchClapMetricHint
                    } else {
                        Key::ReferenceMatchMetricHint
                    },
                )))
                .children(
                    report
                        .changes
                        .iter()
                        .map(|change| div().text_xs().child(change.clone()).into_any_element()),
                );
        }
        let mut actions = div()
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2();
        if state.running.is_some() {
            actions = actions.child(
                button(
                    "reference-match-cancel",
                    self.t(Key::SongSearchCancel),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.reference_match.cancel();
                        this.stop_reference_preview();
                        cx.notify();
                    }),
                )
                .when(last_control == LastControl::Cancel, |this| {
                    this.track_focus(self.modal_focus.reference_match_last())
                }),
            );
        } else {
            actions = actions.child(
                button_enabled(
                    "reference-match-start",
                    self.t(Key::ReferenceMatchStart),
                    ButtonStyle::Primary,
                    ButtonState::available(false, can_start),
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        if can_start {
                            this.start_reference_match(cx);
                        }
                    }),
                )
                .when(last_control == LastControl::Start, |this| {
                    this.track_focus(self.modal_focus.reference_match_last())
                }),
            );
        }
        actions = actions.child(
            button_enabled(
                "reference-match-apply",
                self.t(Key::ReferenceMatchApply),
                ButtonStyle::Normal,
                ButtonState::available(false, can_apply),
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    if can_apply {
                        this.apply_reference_match(cx);
                    }
                }),
            )
            .when(last_control == LastControl::Apply, |this| {
                this.track_focus(self.modal_focus.reference_match_last())
            }),
        );
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.72))
                .occlude()
                .child(
                    div()
                        .debug_selector(|| "reference-match-panel".to_string())
                        .track_focus(self.modal_focus.reference_match())
                        .tab_group()
                        .tab_stop(false)
                        .w(width)
                        .h(viewport.height * 0.92)
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .bg(theme.surface_raised)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(Metrics::RADIUS_LG)
                        .text_color(theme.text)
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .child(self.t(Key::CmdMatchReference)),
                                )
                                .child(
                                    button(
                                        "reference-match-close",
                                        self.t(Key::Close),
                                        ButtonStyle::Normal,
                                        false,
                                        theme.accent,
                                        theme,
                                        cx.listener(|this, _, _, cx| {
                                            this.close_reference_match(cx)
                                        }),
                                    )
                                    .when(
                                        last_control == LastControl::Close,
                                        |this| {
                                            this.track_focus(
                                                self.modal_focus.reference_match_last(),
                                            )
                                        },
                                    ),
                                ),
                        )
                        .child(body)
                        .child(divider(theme))
                        .child(
                            div()
                                .debug_selector(|| "reference-match-footer".to_string())
                                .flex_shrink_0()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(self.audio_match_status(stale))
                                .child(actions)
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.text_muted)
                                        .child(self.t(Key::ReferenceMatchApplyHint)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn audio_match_status(&self, stale: bool) -> Option<AnyElement> {
        let status = self.audio_match_main_status(stale);
        let preview = self.reference_match.preview.as_ref().map(|preview| {
            div()
                .debug_selector(|| "reference-match-preview-status".to_string())
                .flex_shrink_0()
                .text_xs()
                .child(format!(
                    "{} · {}",
                    self.t(if preview.status.is_some() {
                        Key::AudioMatchPreviewPlaying
                    } else {
                        Key::AudioMatchPreviewPreparing
                    }),
                    self.t(match preview.selection {
                        0 => Key::ReferenceMatchSource,
                        1 => Key::ReferenceMatchBefore,
                        _ => Key::ReferenceMatchBest,
                    })
                ))
        });
        if status.is_none() && preview.is_none() {
            return None;
        }
        Some(
            div()
                .id("reference-match-status")
                .debug_selector(|| "reference-match-status".to_string())
                .flex_shrink_0()
                .max_h(px(80.0))
                .min_w_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_2()
                .children(preview)
                .children(status)
                .into_any_element(),
        )
    }

    fn audio_match_main_status(&self, stale: bool) -> Option<AnyElement> {
        let state = &self.reference_match;
        let theme = &self.theme;
        let message = |selector: &'static str, text: String| {
            div()
                .debug_selector(move || selector.to_string())
                .flex_shrink_0()
                .text_xs()
                .child(text)
                .into_any_element()
        };
        let content = if let Some(run) = &state.running {
            let completed = run.completed.load(Ordering::Relaxed);
            let progress = f32::from_bits(run.fraction.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_2()
                .child(message(
                    "reference-match-progress",
                    format!(
                        "{} · {} / {}",
                        self.t(if run.cancel.load(Ordering::Relaxed) {
                            Key::SongSearchCancelling
                        } else if !run.prepared.load(Ordering::Relaxed) {
                            if run.snapshot.objective.uses_clap() {
                                Key::AudioMatchPreparingClap
                            } else {
                                Key::ReferenceMatchPreparing
                            }
                        } else {
                            Key::ReferenceMatchRendering
                        }),
                        completed,
                        run.snapshot.settings.attempts
                    ),
                ))
                .child(
                    div()
                        .flex_shrink_0()
                        .h(px(4.0))
                        .w_full()
                        .bg(theme.border_subtle)
                        .child(div().h_full().w(relative(progress)).bg(theme.accent)),
                )
                .into_any_element()
        } else if state.loading.is_some() {
            message(
                "reference-match-loading",
                self.t(Key::ReferenceMatchLoading).into(),
            )
        } else if let Some(error) = &state.error {
            message(
                "reference-match-error",
                format!("{}: {error}", self.t(Key::ReferenceMatchFailed)),
            )
        } else if stale {
            message(
                "reference-match-stale",
                self.t(Key::ReferenceMatchChanged).into(),
            )
        } else if let Some(comparison) = &state.comparison {
            let rejected = if comparison.report.failed_attempts > 0 {
                format!(
                    " · {}: {}",
                    self.t(Key::ReferenceMatchRejected),
                    comparison.report.failed_attempts,
                )
            } else {
                String::new()
            };
            message(
                "reference-match-complete",
                format!(
                    "{} · {}: {}{rejected}",
                    self.t(if comparison.report.cancelled {
                        Key::SongSearchCancelled
                    } else {
                        Key::SongSearchComplete
                    }),
                    self.t(Key::SongSearchProgress),
                    comparison.report.attempts
                ),
            )
        } else if state.cancelled {
            message(
                "reference-match-cancelled",
                self.t(Key::SongSearchCancelled).into(),
            )
        } else {
            let problem = state.input_problem()?;
            message("reference-match-input-problem", self.t(problem).into())
        };
        Some(content)
    }

    fn audio_match_target(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.reference_match;
        let editable = !state.busy();
        let theme = &self.theme;
        let mut target = div().flex().flex_col().gap_2().child(
            reference_row(self, Key::AudioMatchObjective).children(
                [
                    ("audio-match-acoustic", MatchObjective::AcousticReference),
                    ("audio-match-clap-reference", MatchObjective::ClapReference),
                    ("audio-match-clap-text", MatchObjective::ClapText),
                ]
                .into_iter()
                .map(|(id, objective)| {
                    choice(
                        self,
                        cx,
                        id,
                        self.t(objective.label()),
                        editable,
                        state.objective == objective,
                        move |state| state.objective = objective,
                    )
                }),
            ),
        );
        if state.objective.uses_clap() {
            target = target
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(button_enabled(
                            "audio-match-model",
                            self.t(Key::AudioMatchChooseModel),
                            ButtonStyle::Normal,
                            ButtonState::available(false, editable),
                            theme.accent,
                            theme,
                            cx.listener(|this, _, _, cx| this.choose_audio_match_model(cx)),
                        ))
                        .when_some(state.model_directory.as_ref(), |this, path| {
                            this.child(
                                div()
                                    .min_w_0()
                                    .text_xs()
                                    .truncate()
                                    .child(path.display().to_string()),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::AudioMatchModelHint)),
                );
        }
        if state.objective == MatchObjective::ClapText {
            target = target
                .child(button_enabled(
                    "audio-match-prompt",
                    self.t(Key::AudioMatchPrompt),
                    ButtonStyle::Normal,
                    ButtonState::available(false, editable),
                    theme.accent,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.edit_audio_match_prompt();
                        cx.notify();
                    }),
                ))
                .when(!state.text_prompt.is_empty(), |this| {
                    this.child(
                        div()
                            .debug_selector(|| "audio-match-prompt-text".to_string())
                            .min_w_0()
                            .text_xs()
                            .child(state.text_prompt.clone()),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::AudioMatchPromptHint)),
                );
        }
        target.into_any_element()
    }
}

fn number(value: f64) -> gpui::Div {
    div()
        .min_w(px(44.0))
        .text_center()
        .child(format!("{value:.1}"))
}

fn reference_row(app: &AurisApp, label: Key) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .text_xs()
        .child(
            div()
                .w_full()
                .text_color(app.theme.text_muted)
                .child(app.t(label)),
        )
}

fn choice<I, L, F>(
    app: &AurisApp,
    cx: &mut Context<AurisApp>,
    id: I,
    label: L,
    enabled: bool,
    active: bool,
    update: F,
) -> gpui::Stateful<gpui::Div>
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    F: Fn(&mut ReferenceMatchState) + 'static,
{
    button_enabled(
        id,
        label,
        ButtonStyle::Normal,
        ButtonState::available(active, enabled),
        app.theme.accent,
        &app.theme,
        cx.listener(move |this, _, _, cx| {
            if enabled && !this.reference_match.busy() {
                this.change_reference_settings(|state| update(state));
                cx.notify();
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use gpui::{KeyUpEvent, Keystroke, TestAppContext};

    #[gpui::test]
    fn reverse_tab_skips_disabled_actions_and_reaches_the_last_enabled_choice(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_reference_match(cx));
        crate::harness::paint(&app, cx);

        cx.simulate_keystrokes("shift-tab");
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.modal_focus.reference_match_last().is_focused(window),
                    "reverse Tab reaches the last enabled control"
                );
            });
        });
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("space").unwrap(),
        });

        app.read_with(cx, |this, _| {
            assert!(
                !this.reference_match.settings.arrangement,
                "the last enabled scope choice activates instead of disabled Apply"
            );
        });
    }
}
