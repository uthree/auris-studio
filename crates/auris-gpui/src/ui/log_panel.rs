//! The log panel: what the application has said, where somebody can read it.
//!
//! A DAW fails quietly on purpose — a SoundFont whose file has moved costs one track its sound
//! rather than the session, a plugin the registry does not know is substituted, an engine command
//! dropped under load is dropped. Every one of those is logged, and until this panel the log went
//! to a terminal: which a release build does not open, and which nobody launching an application
//! from an icon has ever looked at. The result was a DAW that told you nothing when a track went
//! silent.

use auris_i18n::Key;
use gpui::{IntoElement, Window, div, prelude::*, px};
use log::Level;

use crate::app::AurisApp;
use crate::logbook::{Entry, book};
use crate::theme::Metrics;
use crate::ui::lint::LogTab;
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
        let tab = self.lint.tab;
        let problems = self.lint.diagnostics().len();
        let problems_label = match problems {
            0 => self.t(Key::ProblemsTab).to_string(),
            count => format!("{} ({count})", self.t(Key::ProblemsTab)),
        };
        let entries = book().entries();
        let empty = entries.is_empty();

        // Newest first, because the reason anybody opened this is the thing that just happened —
        // scrolling to the bottom of five hundred lines to find it is the terminal's behaviour,
        // and a terminal is what this replaces.
        let lines: Vec<atelier::LogLine> = entries.iter().rev().map(log_line).collect();
        let scroll = self.log_scroll.clone();

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
                    .child(button(
                        "log-tab-problems",
                        problems_label,
                        ButtonStyle::Ghost,
                        tab == LogTab::Problems,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.lint.tab = LogTab::Problems;
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "log-tab-log",
                        self.t(Key::ProblemsLogTab),
                        ButtonStyle::Ghost,
                        tab == LogTab::Log,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.lint.tab = LogTab::Log;
                            cx.notify();
                        }),
                    ))
                    .child(div().flex_1().min_w_0())
                    .when(tab == LogTab::Log, |this| {
                        this.child(button(
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
                        ))
                    }),
            )
            .when(tab == LogTab::Problems, |this| {
                this.child(self.render_problems(cx))
            })
            .when(tab == LogTab::Log, |this| {
                this.child(
                    self.scrolling(
                        ScrollPanel::Log,
                        div()
                            .id("log-lines")
                            .debug_selector(|| "log-lines".to_owned())
                            .flex()
                            .flex_col()
                            .p_1()
                            .overflow_y_scroll()
                            // One selection down the whole log: a drag runs across records and
                            // copies them as a terminal would print them.
                            .when(!empty, |this| {
                                this.child(
                                    atelier::LogView::new(LOG_VIEW_ID)
                                        .scroll(&scroll)
                                        .lines(lines),
                                )
                            })
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
            })
    }
}

/// The log view's element id. A record's parts are `log-view-{number}-{part}`, where `part` is
/// `time`, `level`, `source` or `message`, keyed on the record rather than its row, which every
/// new line moves down one.
pub(crate) const LOG_VIEW_ID: &str = "log-view";

/// One record as Atelier's `LogView` draws it: how long after start-up, how bad, who said it,
/// and what it said.
fn log_line(entry: &Entry) -> atelier::LogLine {
    atelier::LogLine::new(entry.number.to_string(), entry.message.clone())
        .time(format!("{}s", entry.seconds))
        .level(log_level(entry.level))
        .source(entry.target.clone())
}

/// `log`'s level as the view names it.
fn log_level(level: Level) -> atelier::LogLevel {
    match level {
        Level::Error => atelier::LogLevel::Error,
        Level::Warn => atelier::LogLevel::Warn,
        Level::Info => atelier::LogLevel::Info,
        Level::Debug => atelier::LogLevel::Debug,
        Level::Trace => atelier::LogLevel::Trace,
    }
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
                log_level(entry.level).word(),
                entry.target,
                entry.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        for level in [
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ] {
            assert_eq!(log_level(level).word(), level.as_str());
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

    #[gpui::test]
    fn the_log_can_be_copied_whole_and_line_by_line(cx: &mut TestAppContext) {
        let _book = crate::logbook::exclusive_book();
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
        harness::select_text(&app, cx, format!("log-view-{number}-message").leak());
        cx.simulate_keystrokes("secondary-c");
        cx.update(|_, cx| {
            let copied = cx.read_from_clipboard().and_then(|item| item.text());
            assert_eq!(copied.as_deref(), Some("the device disappeared"));
        });

        // A drag from the newest record's time down to the end of the older one runs across
        // both, and copies them one to a line as a terminal prints them.
        let older = book().entries()[0].number;
        let from = cx
            .debug_bounds(format!("log-view-{number}-time").leak())
            .expect("the newest record's time is visible");
        let to = cx
            .debug_bounds(format!("log-view-{older}-message").leak())
            .expect("the older record's message is visible");
        let start = from.origin + gpui::point(px(0.5), px(0.5));
        let end = to.bottom_right() - gpui::point(px(0.5), px(0.5));
        harness::press(cx, start);
        harness::paint(&app, cx);
        harness::drag_to(cx, end);
        harness::paint(&app, cx);
        harness::release(cx, end);
        harness::paint(&app, cx);
        cx.simulate_keystrokes("secondary-c");
        cx.update(|_, cx| {
            let copied = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default();
            let lines: Vec<&str> = copied
                .lines()
                .map(|line| line.split_once("s ").expect("seconds first").1)
                .collect();
            assert_eq!(
                lines,
                [
                    "ERROR auris_engine: the device disappeared",
                    "WARN auris_session::session: a font moved",
                ]
            );
        });
    }

    #[gpui::test]
    fn a_narrow_log_keeps_severity_source_and_message_readable(cx: &mut TestAppContext) {
        let _book = crate::logbook::exclusive_book();
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
        let number = book().entries()[0].number;
        for part in ["level", "source", "message"] {
            let selector: &'static str = format!("log-view-{number}-{part}").leak();
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
