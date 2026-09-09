//! Audio objective controls and comparisons of captured renders.

use crate::theme::{Metrics, Theme};
use crate::ui::widgets::{ButtonStyle, button, divider};
use gpui::{
    AnyElement, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px,
    relative,
};

use super::*;

impl AurisApp {
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
        let viewport = window.viewport_size();
        let width = (viewport.width - px(32.0)).max(px(0.0)).min(px(920.0));
        let mut body = div()
            .id("reference-match-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
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
            .child(self.audio_match_target(cx))
            .when(state.objective.needs_reference(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(
                            button(
                                "reference-match-file",
                                self.t(Key::ReferenceMatchChoose),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                theme,
                                cx.listener(|this, _, _, cx| this.choose_reference_audio(cx)),
                            )
                            .when(!editable, |this| this.opacity(0.5)),
                        )
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
                        }),
                )
                .when_some(state.source.as_ref(), |this, source| {
                    this.child(div().text_xs().text_color(theme.text_muted).child(format!(
                        "{}: {:.1} s",
                        self.t(Key::ReferenceMatchFileDuration),
                        source.audio.duration_seconds()
                    )))
                })
            });
        if state.loading.is_some() {
            body = body.child(div().text_xs().child(self.t(Key::ReferenceMatchLoading)));
        }
        body = body
            .child(
                div()
                    .grid()
                    .grid_cols(if width >= px(760.0) { 2 } else { 1 })
                    .gap_3()
                    .child(
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
                            .child(
                                button(
                                    "reference-project-playhead",
                                    self.t(Key::ReferenceMatchPlayhead),
                                    ButtonStyle::Ghost,
                                    false,
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
                                )
                                .when(!editable, |this| this.opacity(0.5)),
                            ),
                    )
                    .when(state.objective.needs_reference(), |this| {
                        this.child(
                            reference_row(self, Key::ReferenceMatchReferenceStart)
                                .child(choice(
                                    self,
                                    cx,
                                    "reference-source-less",
                                    "−",
                                    editable && state.reference_start > 0.0,
                                    false,
                                    |state| {
                                        state.reference_start =
                                            (state.reference_start - 1.0).max(0.0)
                                    },
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
                                )),
                        )
                    })
                    .child(reference_row(self, Key::ReferenceMatchDuration).children(
                        [1usize, 2, 5, 10, 12, 20, 30].into_iter().map(|seconds| {
                            choice(
                                self,
                                cx,
                                ("reference-duration", seconds),
                                seconds.to_string(),
                                editable,
                                state.settings.duration_seconds == seconds as f64,
                                move |state| state.settings.duration_seconds = seconds as f64,
                            )
                        }),
                    ))
                    .child(reference_row(self, Key::SongSearchAttempts).children(
                        [4usize, 8, 16, 32, 64, 128].into_iter().map(|attempts| {
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
                    ))
                    .child(
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
                            )),
                    ),
            )
            .child(
                div()
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
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::ReferenceMatchScope)),
            );
        let stale = state
            .comparison
            .as_ref()
            .is_some_and(|result| !self.match_snapshot_is_current(&result.snapshot));
        let can_preview = state.comparison.is_some() && !stale;
        body = body
            .child(divider(theme))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::ReferenceMatchPreviewHint)),
            )
            .child(
                div()
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
                        .map(|(selection, id, label, enabled)| {
                            button(
                                id,
                                self.t(label),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                theme,
                                cx.listener(move |this, _, _, cx| {
                                    if enabled {
                                        this.preview_reference_match(selection, cx);
                                    }
                                }),
                            )
                            .when(!enabled, |this| this.opacity(0.5))
                            .into_any_element()
                        }),
                    )
                    .child(button(
                        "reference-preview-stop",
                        self.t(Key::ReferenceMatchStop),
                        ButtonStyle::Ghost,
                        false,
                        theme.accent,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.stop_reference_preview();
                            cx.notify();
                        }),
                    )),
            );
        if let Some(run) = &state.running {
            let completed = run.completed.load(Ordering::Relaxed);
            let fraction = f32::from_bits(run.fraction.load(Ordering::Relaxed)).clamp(0.0, 1.0);
            let progress = fraction;
            body = body
                .child(
                    div()
                        .debug_selector(|| "reference-match-progress".to_string())
                        .text_xs()
                        .child(format!(
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
                        )),
                )
                .child(
                    div()
                        .h(px(4.0))
                        .w_full()
                        .bg(theme.border_subtle)
                        .child(div().h_full().w(relative(progress)).bg(theme.accent)),
                );
        }
        if let Some(comparison) = &state.comparison {
            let report = &comparison.report;
            body = body
                .child(divider(theme))
                .child(div().text_xs().child(format!(
                    "{} · {} {}",
                    self.t(if report.cancelled {
                        Key::SongSearchCancelled
                    } else {
                        Key::SongSearchComplete
                    }),
                    report.attempts,
                    self.t(Key::SongSearchProgress)
                )))
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
        } else if state.cancelled && state.running.is_none() {
            body = body.child(div().text_xs().child(self.t(Key::SongSearchCancelled)));
        }
        if stale {
            body = body.child(
                div()
                    .debug_selector(|| "reference-match-stale".to_string())
                    .text_xs()
                    .child(self.t(Key::ReferenceMatchChanged)),
            );
        }
        if let Some(error) = &state.error {
            body = body.child(
                div()
                    .debug_selector(|| "reference-match-error".to_string())
                    .text_xs()
                    .child(format!("{}: {error}", self.t(Key::ReferenceMatchFailed))),
            );
        }
        if editable && let Some(problem) = state.input_problem() {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(problem)),
            );
        }
        let can_start = editable && state.input_problem().is_none();
        let can_apply = editable && can_preview;
        let mut actions = div().flex().flex_wrap().items_center().gap_2();
        if state.running.is_some() {
            actions = actions.child(button(
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
            ));
        } else {
            actions = actions.child(
                button(
                    "reference-match-start",
                    self.t(Key::ReferenceMatchStart),
                    ButtonStyle::Primary,
                    false,
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        if can_start {
                            this.start_reference_match(cx);
                        }
                    }),
                )
                .when(!can_start, |this| this.opacity(0.5)),
            );
        }
        actions = actions.child(
            button(
                "reference-match-apply",
                self.t(Key::ReferenceMatchApply),
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    if can_apply {
                        this.apply_reference_match(cx);
                    }
                }),
            )
            .when(!can_apply, |this| this.opacity(0.5)),
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
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .child(self.t(Key::CmdMatchReference)),
                                )
                                .child(button(
                                    "reference-match-close",
                                    self.t(Key::Close),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    theme,
                                    cx.listener(|this, _, _, cx| this.close_reference_match(cx)),
                                )),
                        )
                        .child(body)
                        .child(divider(theme))
                        .child(actions)
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(self.t(Key::ReferenceMatchApplyHint)),
                        ),
                )
                .into_any_element(),
        )
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
                        .child(
                            button(
                                "audio-match-model",
                                self.t(Key::AudioMatchChooseModel),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                theme,
                                cx.listener(|this, _, _, cx| this.choose_audio_match_model(cx)),
                            )
                            .when(!editable, |this| this.opacity(0.5)),
                        )
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
                .child(
                    button(
                        "audio-match-prompt",
                        self.t(Key::AudioMatchPrompt),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.edit_audio_match_prompt();
                            cx.notify();
                        }),
                    )
                    .when(!editable, |this| this.opacity(0.5)),
                )
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
) -> AnyElement
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    F: Fn(&mut ReferenceMatchState) + 'static,
{
    button(
        id,
        label,
        ButtonStyle::Normal,
        active,
        app.theme.accent,
        &app.theme,
        cx.listener(move |this, _, _, cx| {
            if enabled && !this.reference_match.busy() {
                this.change_reference_settings(|state| update(state));
                cx.notify();
            }
        }),
    )
    .when(!enabled, |this| this.opacity(0.5))
    .into_any_element()
}
