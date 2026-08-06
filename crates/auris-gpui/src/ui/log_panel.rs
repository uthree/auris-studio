//! The log panel: what the application has said, where somebody can read it.
//!
//! A DAW fails quietly on purpose — a SoundFont whose file has moved costs one track its sound
//! rather than the session, a plugin the registry does not know is substituted, an engine command
//! dropped under load is dropped. Every one of those is logged, and until this panel the log went
//! to a terminal: which a release build does not open, and which nobody launching an application
//! from an icon has ever looked at. The result was a DAW that told you nothing when a track went
//! silent.

use auris_i18n::Key;
use gpui::{AnyElement, IntoElement, Window, div, prelude::*, px};
use log::Level;

use crate::app::AurisApp;
use crate::logbook::{Entry, book};
use crate::theme::{Metrics, Theme};
use crate::ui::widgets::{ButtonStyle, button};

impl AurisApp {
    /// Renders the log panel.
    pub(crate) fn render_log(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let entries = book().entries();
        let empty = entries.is_empty();

        let rows: Vec<AnyElement> = entries
            .iter()
            .rev()
            .enumerate()
            .map(|(index, entry)| log_row(index, entry, &theme))
            .collect();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(80.0))
            .bg(theme.surface_sunken)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(Metrics::PANEL_HEADER_HEIGHT)
                    .px_2()
                    .bg(theme.surface_raised)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(div().flex_1().child(self.t(Key::LogPanel)))
                    .child(button(
                        "log-clear",
                        self.t(Key::LogClear),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|_, _, _, cx| {
                            book().clear();
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .id("log-lines")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .p_1()
                    .overflow_y_scroll()
                    // Newest first, because the reason anybody opened this is the thing that just
                    // happened — scrolling to the bottom of five hundred lines to find it is the
                    // terminal's behaviour, and a terminal is what this replaces.
                    .children(rows)
                    .when(empty, |this| {
                        this.child(
                            div()
                                .p_2()
                                .text_xs()
                                .text_color(theme.text_faint)
                                .child(self.t(Key::LogEmpty)),
                        )
                    }),
            )
    }
}

/// One line: how long after start-up, how bad, who said it, and what it said.
fn log_row(index: usize, entry: &Entry, theme: &Theme) -> AnyElement {
    div()
        .id(("log-line", index))
        .flex()
        .items_start()
        .gap_2()
        .px_1p5()
        .py_0p5()
        .text_xs()
        .child(
            div()
                .w(px(44.0))
                .flex_shrink_0()
                .text_color(theme.text_faint)
                .child(format!("{}s", entry.seconds)),
        )
        .child(
            div()
                .w(px(52.0))
                .flex_shrink_0()
                .text_color(level_colour(entry.level, theme))
                .child(level_word(entry.level)),
        )
        .child(
            div()
                .w(px(180.0))
                .flex_shrink_0()
                .text_color(theme.text_faint)
                .child(entry.target.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme.text)
                .child(entry.message.clone()),
        )
        .into_any_element()
}

/// The colour a level is drawn in.
///
/// Only the two that mean something get one. Colouring information and debug as well would make
/// the panel a rainbow in which the one red line nobody is meant to miss is just another colour.
fn level_colour(level: Level, theme: &Theme) -> gpui::Hsla {
    match level {
        Level::Error => theme.danger,
        Level::Warn => theme.warning,
        _ => theme.text_muted,
    }
}

/// The word for a level.
///
/// Not translated. These are `log`'s own five names, they appear in `RUST_LOG` and in every
/// terminal the same records also went to, and a panel that called a warning something else would
/// be a panel somebody had to learn a second vocabulary for.
fn level_word(level: Level) -> &'static str {
    match level {
        Level::Error => "ERROR",
        Level::Warn => "WARN",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_has_a_word_and_it_is_the_one_rust_log_uses() {
        // A panel that renamed these would be a second vocabulary to learn, for records that also
        // went to a terminal under the first one.
        assert_eq!(level_word(Level::Error), "ERROR");
        assert_eq!(level_word(Level::Warn), "WARN");
        assert_eq!(level_word(Level::Info), "INFO");
        assert_eq!(level_word(Level::Debug), "DEBUG");
        assert_eq!(level_word(Level::Trace), "TRACE");
    }

    #[test]
    fn only_a_warning_or_an_error_is_coloured() {
        let theme = Theme::default();
        assert_eq!(level_colour(Level::Error, &theme), theme.danger);
        assert_eq!(level_colour(Level::Warn, &theme), theme.warning);
        for quiet in [Level::Info, Level::Debug, Level::Trace] {
            assert_eq!(
                level_colour(quiet, &theme),
                theme.text_muted,
                "{quiet} should not have a colour of its own"
            );
        }
    }
}
