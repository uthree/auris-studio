//! The log panel: what the application has said, where somebody can read it.
//!
//! A DAW fails quietly on purpose — a SoundFont whose file has moved costs one track its sound
//! rather than the session, a plugin the registry does not know is substituted, an engine command
//! dropped under load is dropped. Every one of those is logged, and until this panel the log went
//! to a terminal: which a release build does not open, and which nobody launching an application
//! from an icon has ever looked at. The result was a DAW that told you nothing when a track went
//! silent.

use auris_i18n::Key;
use gpui::{AnyElement, IntoElement, SharedString, Window, div, prelude::*, px};
use log::Level;

use crate::app::AurisApp;
use crate::logbook::{Entry, book};
use crate::theme::{Metrics, Theme};
use crate::ui::scrollbars::ScrollPanel;
use crate::ui::widgets::{ButtonState, ButtonStyle, button, button_enabled};

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
            // The mixer's note, for the same reason: a flex item's `min-width` is `auto`, which
            // is its content's width, and a log holding one long path claims the width of that
            // path. The dock hands it over and clips it, so the Clear button at the end of the
            // strip ends up somewhere no pointer can reach.
            .min_w_0()
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.t(Key::LogPanel)),
                    )
                    .child(button(
                        "log-report",
                        self.t(Key::IssueReport),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.open_issue_report(None, cx)),
                    ))
                    .child(button_enabled(
                        "log-copy-all",
                        self.t(Key::LogCopyAll),
                        ButtonStyle::Normal,
                        ButtonState::available(false, !empty),
                        theme.accent,
                        &theme,
                        cx.listener(|_, _, _, cx| {
                            // Read again at the click rather than captured at the frame: a line
                            // logged in between is one somebody investigating wants too.
                            let text = log_text(&book().entries());
                            if !text.is_empty() {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                            }
                        }),
                    ))
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
                self.scrolling(
                    ScrollPanel::Log,
                    div()
                        .id("log-lines")
                        .debug_selector(|| "log-lines".to_owned())
                        .flex()
                        .flex_col()
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
                    cx,
                ),
            )
    }
}

/// One line: how long after start-up, how bad, who said it, and what it said.
fn log_row(index: usize, entry: &Entry, theme: &Theme) -> AnyElement {
    div()
        .id(("log-line", index))
        .flex()
        .min_w_0()
        .items_start()
        .gap_2()
        .px_1p5()
        .py_0p5()
        .text_xs()
        .child(
            div()
                .w(px(32.0))
                .flex_shrink_0()
                .text_color(theme.text_faint)
                .child(format!("{}s", entry.seconds)),
        )
        .child(
            div()
                .w(px(44.0))
                .flex_shrink_0()
                .text_color(level_colour(entry.level, theme))
                .debug_selector(move || format!("log-level-{index}"))
                .child(level_word(entry.level)),
        )
        .child(
            div()
                .id(("log-target", index))
                .w(px(120.0))
                .min_w(px(24.0))
                .flex_shrink()
                .truncate()
                .text_color(theme.text_faint)
                .debug_selector(move || format!("log-target-{index}"))
                .child(entry.target.clone())
                .tooltip(crate::ui::tooltip::keyed_tip(
                    entry.target.clone(),
                    "",
                    theme,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .text_color(theme.text)
                .debug_selector(move || format!("log-message-{index}"))
                // Keyed on the record rather than the row, which every new line moves down one.
                .child(
                    atelier::SelectableText::new(
                        SharedString::from(format!("log-text-{}", entry.number)),
                        entry.message.clone(),
                    )
                    .size(atelier::TextSize::XSmall),
                ),
        )
        .into_any_element()
}

/// The whole log as text to paste into a report, oldest first as a terminal would have it.
///
/// One line per record, `12s WARN auris_session::session: message`, with the module in full
/// where the panel truncates it — the panel is read on a screen and this is read in an issue.
fn log_text(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "{}s {} {}: {}",
                entry.seconds,
                level_word(entry.level),
                entry.target,
                entry.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    use crate::dock::Panel;
    use crate::harness;
    use gpui::{TestAppContext, size};

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

    #[test]
    fn the_copied_log_reads_oldest_first_with_every_module_in_full() {
        let entry = |number, level, target: &str, message: &str| Entry {
            level,
            target: target.into(),
            message: message.into(),
            number,
            seconds: number * 3,
        };
        let entries = [
            entry(0, Level::Info, "auris_gpui", "started"),
            entry(
                1,
                Level::Warn,
                "auris_session::session::plugin",
                "no font\nfor track 2",
            ),
        ];
        assert_eq!(
            log_text(&entries),
            "0s INFO auris_gpui: started\n3s WARN auris_session::session::plugin: no font\nfor track 2"
        );
        assert_eq!(log_text(&[]), "");
    }

    /// The log book is one per process, and the test threads share it. A test that writes entries
    /// and then reads them back holds this for its whole length, starting and ending with an
    /// empty book, or another such test clears its entries out from under it.
    struct ExclusiveLog {
        _held: std::sync::MutexGuard<'static, ()>,
    }

    impl ExclusiveLog {
        fn take() -> Self {
            static LOG: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let guard = LOG
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            book().clear();
            Self { _held: guard }
        }
    }

    impl Drop for ExclusiveLog {
        fn drop(&mut self) {
            book().clear();
        }
    }

    #[gpui::test]
    fn the_log_can_be_copied_whole_and_line_by_line(cx: &mut TestAppContext) {
        let _log = ExclusiveLog::take();
        let (app, cx) = harness::open(cx);
        book().push(Level::Warn, "auris_session::session", "a font moved".into());
        book().push(
            Level::Error,
            "auris_engine",
            "the device disappeared".into(),
        );
        app.update(cx, |this, _| this.show_panel(Panel::Log));
        harness::resize(&app, cx, size(px(1200.0), px(800.0)));

        harness::click("log-copy-all", cx);
        cx.update(|_, cx| {
            let copied = cx.read_from_clipboard().and_then(|item| item.text());
            // The seconds are since the book was created, which another test may have done.
            let lines: Vec<String> = copied
                .unwrap_or_default()
                .lines()
                .map(|line| line.split_once("s ").expect("seconds first").1.to_owned())
                .collect();
            assert_eq!(
                lines,
                [
                    "WARN auris_session::session: a font moved",
                    "ERROR auris_engine: the device disappeared",
                ]
            );
        });

        // Newest first on screen, so row 0 is the error.
        let number = book().entries()[1].number;
        harness::select_text(&app, cx, format!("log-text-{number}").leak());
        cx.simulate_keystrokes("secondary-c");
        cx.update(|_, cx| {
            let copied = cx.read_from_clipboard().and_then(|item| item.text());
            assert_eq!(copied.as_deref(), Some("the device disappeared"));
        });
    }

    #[gpui::test]
    fn a_narrow_log_keeps_severity_source_and_message_readable(cx: &mut TestAppContext) {
        let _log = ExclusiveLog::take();
        let (app, cx) = harness::open(cx);
        book().push(
            Level::Error,
            "auris_session::session::plugin::a_very_long_provider_name",
            "The selected plugin could not be loaded. Choose another sound and try again.".into(),
        );
        app.update(cx, |this, _| this.show_panel(Panel::Log));
        let viewport = size(px(640.0), px(480.0));
        harness::resize(&app, cx, viewport);

        let lines = cx.debug_bounds("log-lines").expect("the log is visible");
        for selector in ["log-level-0", "log-target-0", "log-message-0"] {
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("`{selector}` is visible"));
            assert!(bounds.size.width > px(0.0));
            assert!(bounds.left() >= lines.left());
            assert!(bounds.right() <= lines.right());
            assert!(bounds.top() >= lines.top());
            assert!(bounds.bottom() <= lines.bottom());
        }
    }
}
