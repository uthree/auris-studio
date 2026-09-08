//! The modal progress display for the background song-writing workflow.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Instant;

use auris_i18n::Key;
use gpui::{IntoElement, Window, div, prelude::*, px, relative};

use crate::app::AurisApp;
use crate::theme::{Metrics, Theme};

/// The current stage of song creation, read on the window's regular repaint tick.
pub(crate) struct ComposeProgressState {
    /// Localized description of the work currently running.
    pub(crate) stage: Key,
    /// A source or measurement name that further identifies the current work.
    pub(crate) detail: Option<String>,
    /// A measurable stage's completed fraction, stored as `f32` bits by the worker.
    /// `None` draws an animated bar without implying a known completion percentage.
    pub(crate) progress: Option<Arc<AtomicU32>>,
    /// Animation origin for stages whose amount of work cannot be measured in advance.
    pub(crate) started_at: Instant,
}

impl ComposeProgressState {
    fn fraction(&self) -> Option<f32> {
        self.progress.as_ref().map(|progress| {
            let fraction = f32::from_bits(progress.load(Ordering::Relaxed));
            if fraction.is_finite() {
                // Rendering can finish before the final measurement and document handoff.
                // The modal disappears when all of those have succeeded.
                fraction.clamp(0.0, 0.99)
            } else {
                0.0
            }
        })
    }
}

impl AurisApp {
    /// Keeps the active stage and its progress visible above the song sheet and source chooser.
    pub(crate) fn render_compose_progress(
        &self,
        window: &Window,
    ) -> Option<impl IntoElement + use<>> {
        let progress = self.compose_progress.as_ref()?;
        let theme = self.theme.clone();
        let fraction = progress.fraction();
        let travel = (progress.started_at.elapsed().as_secs_f32() * 2.4).sin() * 0.5 + 0.5;
        let width = (window.viewport_size().width - px(40.0))
            .max(px(0.0))
            .min(px(460.0));
        Some(
            div()
                .id("compose-progress-overlay")
                .debug_selector(|| "compose-progress-overlay".to_owned())
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(Theme::translucent(theme.background, 0.72))
                .occlude()
                .child(
                    div()
                        .id("compose-progress-panel")
                        .debug_selector(|| "compose-progress-panel".to_owned())
                        .w(width)
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded(Metrics::RADIUS_LG)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface_raised)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.text)
                                .child(self.t(Key::ComposeProgressTitle)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_3()
                                .text_sm()
                                .child(
                                    div()
                                        .id("compose-progress-stage")
                                        .debug_selector(|| "compose-progress-stage".to_owned())
                                        .flex_1()
                                        .min_w_0()
                                        .child(self.t(progress.stage)),
                                )
                                .children(fraction.map(|fraction| {
                                    div()
                                        .id("compose-progress-percentage")
                                        .debug_selector(|| "compose-progress-percentage".to_owned())
                                        .flex_shrink_0()
                                        .text_color(theme.text_muted)
                                        .child(format!("{:.0}%", (fraction * 100.0).floor()))
                                })),
                        )
                        .child(
                            div()
                                .id("compose-progress-bar")
                                .debug_selector(|| "compose-progress-bar".to_owned())
                                .relative()
                                .w_full()
                                .h(px(8.0))
                                .rounded(Metrics::RADIUS_SM)
                                .overflow_hidden()
                                .bg(theme.surface_sunken)
                                .child(
                                    div()
                                        .absolute()
                                        .left(relative(if fraction.is_some() {
                                            0.0
                                        } else {
                                            travel * 0.68
                                        }))
                                        .h_full()
                                        .w(relative(fraction.unwrap_or(0.32)))
                                        .rounded(Metrics::RADIUS_SM)
                                        .bg(theme.accent),
                                ),
                        )
                        .children(progress.detail.as_ref().map(|detail| {
                            div()
                                .id("compose-progress-detail")
                                .debug_selector(|| "compose-progress-detail".to_owned())
                                .w_full()
                                .min_w_0()
                                .h(px(16.0))
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(crate::ui::widgets::bounded_picker_label(detail.clone()))
                        }))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(self.t(Key::ComposeProgressHint)),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    use crate::actions;
    use crate::harness::{click, open, paint};
    use crate::ui::text_field::HasTextField;

    fn running(progress: Option<Arc<AtomicU32>>) -> ComposeProgressState {
        ComposeProgressState {
            stage: Key::ComposeProgressWriting,
            detail: None,
            progress,
            started_at: Instant::now(),
        }
    }

    #[gpui::test]
    fn progress_blocks_covered_input_and_document_actions_until_completion(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        let (before, draft, query) = app.update(cx, |this, cx| {
            this.session
                .add_default_instrument_track("Keep this track")
                .unwrap();
            this.open_song_sheet();
            this.open_song_library(0, cx);
            let browser = this.song_library.as_mut().unwrap();
            browser.search = crate::ui::text_field::TextField::new("Piano".to_string());
            browser.search.caret_to_end();
            let query = browser.search.clone();
            this.compose_progress = Some(running(None));
            (this.project().clone(), this.song_sheet.clone(), query)
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("compose-progress-overlay").is_some());
        assert!(cx.debug_bounds("compose-progress-bar").is_some());
        assert!(cx.debug_bounds("compose-progress-percentage").is_none());
        cx.simulate_input("hidden");
        cx.simulate_keystrokes("escape left backspace delete secondary-a tab enter");
        cx.dispatch_action(actions::AddInstrumentTrack);
        cx.dispatch_action(actions::NewProject);
        cx.dispatch_action(actions::Undo);
        click("song-sheet-cancel", cx);
        app.update(cx, |this, _| {
            assert!(this.compose_progress.is_some());
            assert!(this.keys_are_claimed());
            assert!(this.readable_field().is_none());
            assert!(this.field().is_none());
            assert_eq!(this.project(), &before);
            assert_eq!(this.song_sheet, draft);
            assert_eq!(this.song_library.as_ref().unwrap().search, query);
            this.compose_progress = None;
        });
        paint(&app, cx);
        cx.simulate_input("s");
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.song_library.as_ref().unwrap().search.content(),
                "Pianos"
            );
        });
    }

    #[gpui::test]
    fn measured_stages_show_progress_but_wait_for_the_final_handoff(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let fraction = Arc::new(AtomicU32::new(0.5_f32.to_bits()));
        app.update(cx, |this, _| {
            let mut progress = running(Some(Arc::clone(&fraction)));
            progress.detail = Some("音量を測定中のとても長い日本語の音源名🎵".repeat(20));
            this.compose_progress = Some(progress);
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("compose-progress-percentage").is_some());
        assert!(cx.debug_bounds("compose-progress-detail").is_some());
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.compose_progress.as_ref().unwrap().fraction(),
                Some(0.5)
            );
        });
        fraction.store(1.0_f32.to_bits(), Ordering::Relaxed);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.compose_progress.as_ref().unwrap().fraction(),
                Some(0.99)
            );
            assert!(this.keys_are_claimed());
        });
    }
}
