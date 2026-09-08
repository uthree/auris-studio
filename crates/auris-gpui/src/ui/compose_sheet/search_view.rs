//! Search controls and result feedback inside the song sheet's scrolling body.

use auris_i18n::Key;
use auris_session::composition_search::SearchMethod;
use gpui::{AnyElement, Context, ElementId, SharedString, div, prelude::*, px, relative};

use crate::app::AurisApp;
use crate::theme::Metrics;
use crate::ui::widgets::{ButtonStyle, button, divider};

use super::dials::{SongDials, song_spec};
use super::lyrics::{section_label, sections_in_form_order};
use super::search_settings::{SearchSettings, searchable_part};

/// Presentation snapshot supplied by the background-search controller.
#[derive(Clone, Debug, Default)]
pub(crate) struct SearchViewStatus {
    /// A worker is composing or waiting to finish its current attempt.
    pub running: bool,
    /// Cancellation has been requested but the worker has not returned yet.
    pub cancelling: bool,
    /// Number of attempts completed in the captured run.
    pub completed: usize,
    /// Attempt limit of the captured run.
    pub budget: usize,
    /// Written notes per bar of the best candidate so far.
    pub best_density: Option<f64>,
    /// Density target captured when the run started.
    pub target: Option<f64>,
    /// Zero-based identifier of the best candidate.
    pub best_candidate: Option<usize>,
    /// The completed run returned after cancellation.
    pub cancelled: bool,
    /// The completed result no longer matches the sheet, settings, or document.
    pub stale: bool,
    /// Failure description for a run which could not start or complete.
    pub error: Option<String>,
    /// The best captured score can still be applied to this document.
    pub can_apply: bool,
}

impl AurisApp {
    /// Draws independently editable search settings and the captured result's measurements.
    pub(crate) fn render_song_search(
        &self,
        dials: &SongDials,
        status: SearchViewStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let settings = &self.composition_search.settings;
        let editable = !status.running;
        let spec = song_spec(dials);
        let mut rows = vec![
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(self.t(Key::SongSearchHint))
                .into_any_element(),
        ];
        let methods = [
            (SearchMethod::Random, Key::SongSearchRandom),
            (SearchMethod::HillClimb, Key::SongSearchHill),
        ];
        rows.push(
            search_row(self, Key::SongSearchMethod)
                .children(
                    methods
                        .into_iter()
                        .enumerate()
                        .map(|(index, (method, label))| {
                            search_choice(
                                self,
                                cx,
                                ("song-search-method", index),
                                self.t(label),
                                settings.algorithm == method,
                                editable,
                                move |settings| settings.algorithm = method,
                            )
                        }),
                )
                .into_any_element(),
        );
        rows.push(
            search_row(self, Key::SongSearchAttempts)
                .children([8usize, 24, 64, 128].into_iter().map(|budget| {
                    search_choice(
                        self,
                        cx,
                        ("song-search-budget", budget),
                        budget.to_string(),
                        settings.attempt_budget == budget,
                        editable,
                        move |settings| settings.attempt_budget = budget,
                    )
                }))
                .into_any_element(),
        );
        rows.push(
            search_row(self, Key::SongSearchTarget)
                .child(search_choice(
                    self,
                    cx,
                    "song-search-target-less",
                    "−",
                    false,
                    editable && settings.target_notes_per_bar > 1.0,
                    |settings| {
                        settings.target_notes_per_bar =
                            (settings.target_notes_per_bar - 1.0).max(1.0)
                    },
                ))
                .child(
                    div()
                        .min_w(px(40.0))
                        .text_center()
                        .child(format!("{:.0}", settings.target_notes_per_bar)),
                )
                .child(search_choice(
                    self,
                    cx,
                    "song-search-target-more",
                    "+",
                    false,
                    editable && settings.target_notes_per_bar < 256.0,
                    |settings| {
                        settings.target_notes_per_bar =
                            (settings.target_notes_per_bar + 1.0).min(256.0)
                    },
                ))
                .children([8usize, 12, 24, 48].into_iter().map(|target| {
                    search_choice(
                        self,
                        cx,
                        ("song-search-target", target),
                        target.to_string(),
                        settings.target_notes_per_bar == target as f64,
                        editable,
                        move |settings| settings.target_notes_per_bar = target as f64,
                    )
                }))
                .into_any_element(),
        );
        rows.push(
            search_row(self, Key::SongSearchSeed)
                .child(search_choice(
                    self,
                    cx,
                    "song-search-seed-less",
                    "−",
                    false,
                    editable && settings.search_seed > 0,
                    |settings| settings.search_seed = settings.search_seed.saturating_sub(1),
                ))
                .child(
                    div()
                        .min_w(px(40.0))
                        .text_center()
                        .child(settings.search_seed.to_string()),
                )
                .child(search_choice(
                    self,
                    cx,
                    "song-search-seed-more",
                    "+",
                    false,
                    editable && settings.search_seed < u64::MAX,
                    |settings| settings.search_seed = settings.search_seed.saturating_add(1),
                ))
                .child(div().text_color(theme.text_muted).child(format!(
                    "{}: {}",
                    self.t(Key::SongSearchCompositionSeed),
                    dials.seed
                )))
                .into_any_element(),
        );
        let part_choices: Vec<_> = spec
            .parts
            .iter()
            .enumerate()
            .filter(|(_, part)| searchable_part(&spec, part))
            .map(|(index, part)| {
                let name = part.name.clone();
                search_choice(
                    self,
                    cx,
                    ("song-search-part", index),
                    name.clone(),
                    settings.part.as_ref() == Some(&name),
                    editable,
                    move |settings| settings.part = Some(name.clone()),
                )
            })
            .collect();
        rows.push(
            search_row(self, Key::SongSearchPart)
                .child(search_choice(
                    self,
                    cx,
                    "song-search-part-fixed",
                    self.t(Key::SongSearchFixed),
                    settings.part.is_none(),
                    editable,
                    |settings| settings.part = None,
                ))
                .children(part_choices)
                .into_any_element(),
        );
        let section_choices: Vec<_> = sections_in_form_order(dials)
            .into_iter()
            .map(|index| {
                let name = dials.sections[index].name.clone();
                search_choice(
                    self,
                    cx,
                    ("song-search-section", index),
                    section_label(self, &name),
                    settings.section.as_ref() == Some(&name),
                    editable,
                    move |settings| settings.section = Some(name.clone()),
                )
            })
            .collect();
        rows.push(
            search_row(self, Key::SongSearchSection)
                .child(search_choice(
                    self,
                    cx,
                    "song-search-section-fixed",
                    self.t(Key::SongSearchFixed),
                    settings.section.is_none(),
                    editable,
                    |settings| settings.section = None,
                ))
                .children(section_choices)
                .into_any_element(),
        );
        rows.push(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(self.t(Key::SongSearchBounds))
                .into_any_element(),
        );

        let has_parameter = settings.part.is_some() || settings.section.is_some();
        if !has_parameter {
            rows.push(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SongSearchChooseParameter))
                    .into_any_element(),
            );
        }
        let has_run = status.running || status.completed > 0 || status.cancelled;
        if has_run {
            let title = if status.cancelling {
                Key::SongSearchCancelling
            } else if status.running {
                Key::SongSearchRunning
            } else if status.cancelled {
                Key::SongSearchCancelled
            } else {
                Key::SongSearchComplete
            };
            rows.push(divider(theme).into_any_element());
            rows.push(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(self.t(title))
                    .child(format!(
                        "{}: {} / {}",
                        self.t(Key::SongSearchProgress),
                        status.completed,
                        status.budget
                    ))
                    .into_any_element(),
            );
            rows.push(
                div()
                    .debug_selector(|| "song-search-progress".to_string())
                    .w_full()
                    .h(px(4.0))
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.border_subtle)
                    .child(
                        div()
                            .h_full()
                            .rounded(Metrics::RADIUS_SM)
                            .bg(theme.accent)
                            .w(relative(
                                status.completed as f32 / status.budget.max(1) as f32,
                            )),
                    )
                    .into_any_element(),
            );
        }
        if let Some(density) = status.best_density {
            let target = status.target.unwrap_or(settings.target_notes_per_bar);
            rows.push(
                div()
                    .debug_selector(|| "song-search-best".to_string())
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .text_xs()
                    .child(format!("{}: {:.2}", self.t(Key::SongSearchBest), density))
                    .child(format!(
                        "{}: {:.2}",
                        self.t(Key::SongSearchDistance),
                        (density - target).abs()
                    ))
                    .when_some(status.best_candidate, |this, candidate| {
                        this.child(format!(
                            "{}: {}",
                            self.t(Key::SongSearchCandidate),
                            candidate + 1
                        ))
                    })
                    .into_any_element(),
            );
        } else if !status.running && has_run && status.error.is_none() {
            rows.push(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SongSearchNoCandidate))
                    .into_any_element(),
            );
        }
        if let Some(error) = status.error {
            rows.push(
                div()
                    .debug_selector(|| "song-search-error".to_string())
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(format!("{}: {}", self.t(Key::SongSearchFailed), error))
                    .into_any_element(),
            );
        }
        if status.stale {
            rows.push(
                div()
                    .debug_selector(|| "song-search-stale".to_string())
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SongSearchChanged))
                    .into_any_element(),
            );
        }
        let mut actions = div().flex().flex_wrap().items_center().gap_2();
        if status.running {
            actions = actions.child(
                button(
                    "song-search-cancel",
                    self.t(Key::SongSearchCancel),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    theme,
                    cx.listener(|this, _, _, cx| this.cancel_song_search(cx)),
                )
                .when(status.cancelling, |this| this.opacity(0.5)),
            );
        } else {
            actions = actions.child(
                button(
                    "song-search-start",
                    self.t(Key::SongSearchStart),
                    ButtonStyle::Primary,
                    false,
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        if has_parameter {
                            this.start_song_search(cx);
                        }
                    }),
                )
                .when(!has_parameter, |this| this.opacity(0.5)),
            );
        }
        let can_apply = status.can_apply;
        actions = actions.child(
            button(
                "song-search-apply",
                self.t(Key::SongSearchApply),
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    if can_apply {
                        this.apply_song_search(cx);
                    }
                }),
            )
            .when(!can_apply, |this| this.opacity(0.5)),
        );
        rows.push(actions.into_any_element());
        rows.push(
            div()
                .text_xs()
                .text_color(theme.text_muted)
                .child(self.t(Key::SongSearchApplyHint))
                .into_any_element(),
        );

        div()
            .debug_selector(|| "song-search-panel".to_string())
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .min_w_0()
            .border_1()
            .border_color(theme.border)
            .rounded(Metrics::RADIUS_SM)
            .text_color(theme.text)
            .children(rows)
            .into_any_element()
    }
}

fn search_row(app: &AurisApp, label: Key) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .text_xs()
        .child(
            div()
                .min_w(px(156.0))
                .text_color(app.theme.text_muted)
                .child(app.t(label)),
        )
}

fn search_choice<I, L, F>(
    app: &AurisApp,
    cx: &mut Context<AurisApp>,
    id: I,
    label: L,
    active: bool,
    enabled: bool,
    update: F,
) -> AnyElement
where
    I: Into<ElementId>,
    L: Into<SharedString>,
    F: Fn(&mut SearchSettings) + 'static,
{
    button(
        id,
        label,
        ButtonStyle::Normal,
        active,
        app.theme.accent,
        &app.theme,
        cx.listener(move |this, _, _, cx| {
            if enabled {
                update(&mut this.composition_search.settings);
                cx.notify();
            }
        }),
    )
    .max_w(px(220.0))
    .overflow_hidden()
    .when(!enabled, |this| this.opacity(0.5))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use auris_i18n::Language;
    use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, size};

    use super::*;
    use crate::harness::{click, open, paint, resize};

    #[gpui::test]
    fn search_controls_change_settings_without_rewriting_the_song(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| this.open_song_sheet());
        paint(&app, cx);
        click("song-search-toggle", cx);
        paint(&app, cx);
        let song = app.read_with(cx, |this, _| this.song_sheet.clone());
        click("song-search-method-0", cx);
        click("song-search-budget-8", cx);
        click("song-search-target-less", cx);
        click("song-search-seed-more", cx);
        paint(&app, cx);
        let before_scroll = app.read_with(cx, |this, _| {
            let settings = &this.composition_search.settings;
            assert_eq!(settings.algorithm, SearchMethod::Random);
            assert_eq!(settings.attempt_budget, 8);
            assert_eq!(settings.target_notes_per_bar, 23.0);
            assert_eq!(settings.search_seed, 43);
            assert_eq!(this.song_sheet, song);
            settings.clone()
        });
        let control = cx.debug_bounds("song-search-target-more").unwrap();
        cx.simulate_event(ScrollWheelEvent {
            position: control.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-80.0))),
            ..Default::default()
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.composition_search.settings, before_scroll);
            assert_eq!(this.song_sheet, song);
        });
        click("song-search-toggle", cx);
        paint(&app, cx);
        click("song-search-toggle", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.composition_search.settings, before_scroll);
        });
    }

    #[gpui::test]
    fn search_controls_wrap_in_both_languages_and_require_a_parameter(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            this.open_song_sheet();
            this.toggle_song_search(cx);
        });
        for language in [Language::English, Language::Japanese] {
            app.update(cx, |this, _| this.language = language);
            for width in [900.0, 640.0] {
                resize(&app, cx, size(px(width), px(1000.0)));
                for selector in [
                    "song-search-panel",
                    "song-search-target-more",
                    "song-search-part-fixed",
                    "song-search-section-fixed",
                    "song-search-start",
                    "song-search-apply",
                ] {
                    let bounds = cx.debug_bounds(selector).expect("search control is drawn");
                    assert!(
                        bounds.left() >= px(0.0) && bounds.right() <= px(width),
                        "{selector} must fit horizontally at {width}: {bounds:?}"
                    );
                }
            }
        }
        resize(&app, cx, size(px(1280.0), px(1080.0)));
        click("song-search-part-fixed", cx);
        click("song-search-section-fixed", cx);
        paint(&app, cx);
        let before = app.read_with(cx, |this, _| this.project().clone());
        click("song-search-start", cx);
        app.read_with(cx, |this, _| {
            assert!(!this.song_search_view_status().running);
            assert_eq!(*this.project(), before);
        });
    }
}
