//! The transport bar across the top of the window.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{Axis, IntoElement, Window, div, prelude::*, px};

use crate::app::{AurisApp, Drag, EditorTab};
use crate::theme::Metrics;
use crate::ui::icons::Icon;
use crate::ui::widgets::{
    ButtonStyle, button, db_to_meter_position, icon_button, level_meter, readout,
};

/// Grid divisions offered in the transport bar, as a fraction of a quarter note.
const GRID_CHOICES: [(&str, i64); 6] = [
    ("1/1", TICKS_PER_QUARTER * 4),
    ("1/2", TICKS_PER_QUARTER * 2),
    ("1/4", TICKS_PER_QUARTER),
    ("1/8", TICKS_PER_QUARTER / 2),
    ("1/16", TICKS_PER_QUARTER / 4),
    ("1/32", TICKS_PER_QUARTER / 8),
];

impl AurisApp {
    /// Renders the transport bar.
    pub(crate) fn render_transport(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        let playing = self.is_playing();
        let looping = self.project().loop_enabled;
        let playhead = self.playhead_ticks();
        let (bar, beat, tick) = self
            .project()
            .tempo_map
            .bar_beat_at(playhead, self.project().time_signature);
        let seconds = self.project().tempo_map.ticks_to_seconds(playhead);
        let bpm = self.project().bpm();
        let grid_label = self.grid_label();
        let master_db = gain_to_db(self.master_level());
        let master_gain_db = self.project().master.gain_db;
        let editor = self.editor;
        let editor_open = self.panels.editor_visible;
        let inspector_open = self.panels.inspector_visible;

        // Three columns of equal weight, so the middle one lands on the window's centre line
        // however wide the sides grow. Every hardware transport and every DAW puts the
        // controls and the position readout there; anchoring them left makes the eye hunt for
        // the playhead position on a wide window.
        div()
            .flex()
            .items_center()
            .h(Metrics::TRANSPORT_HEIGHT)
            .px_3()
            .bg(theme.surface_raised)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    // Export lives in the File menu, with the other commands that write a file.
                    // A transport bar is for the transport; a button that opens a save dialog was
                    // the widest thing on it and the least often pressed.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_faint)
                                    .child(self.t(Key::Grid)),
                            )
                            .child(button(
                                "grid",
                                grid_label,
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.cycle_grid();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        // The arrangement's own zoom, next to the grid because both are about
                        // how finely the timeline reads rather than about what it contains.
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_faint)
                                    .child(self.t(Key::Zoom)),
                            )
                            .child(self.zoom_slider("timeline-zoom", cx)),
                    ),
            )
            .child(
                // Logic stacks the transport over its readouts rather than running them along
                // one line, which keeps the buttons and the numbers on the window's centre line
                // instead of pushing one of them off it.
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.0))
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(icon_button(
                                "rtz",
                                Icon::ToStart,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.seek(Ticks::ZERO);
                                    cx.notify();
                                }),
                            ))
                            .child(icon_button(
                                "stop",
                                Icon::Stop,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.session.stop();
                                    this.seek(Ticks::ZERO);
                                    cx.notify();
                                }),
                            ))
                            .child(icon_button(
                                "play",
                                if playing { Icon::Pause } else { Icon::Play },
                                playing,
                                theme.playing,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_play();
                                    cx.notify();
                                }),
                            ))
                            .child(icon_button(
                                "loop",
                                Icon::Loop,
                                looping,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_loop();
                                    cx.notify();
                                }),
                            )),
                    )
                    // Musical position, wall-clock position and tempo: the readouts every DAW
                    // shows, side by side under the buttons they describe.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(readout(
                                self.t(Key::Position),
                                format!("{bar}.{beat}.{:03}", tick / 10),
                                Some(seconds.format_clock().into()),
                                px(118.0),
                                &theme,
                            ))
                            .child(self.render_tempo_control(bpm, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_end()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            // Clicking the tab you are already on hides the panel, which is
                            // how every editor's sidebar toggles behave.
                            .child(button(
                                "tab-roll",
                                self.t(Key::PianoRoll),
                                ButtonStyle::Normal,
                                editor_open && editor == EditorTab::PianoRoll,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.show_editor_tab(EditorTab::PianoRoll);
                                    cx.notify();
                                }),
                            ))
                            .child(button(
                                "tab-mixer",
                                self.t(Key::Mixer),
                                ButtonStyle::Normal,
                                editor_open && editor == EditorTab::Mixer,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.show_editor_tab(EditorTab::Mixer);
                                    cx.notify();
                                }),
                            ))
                            .child(button(
                                "tab-inspector",
                                self.t(Key::Inspector),
                                ButtonStyle::Normal,
                                inspector_open,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_inspector();
                                    cx.notify();
                                }),
                            )),
                    )
                    // Master level, always visible so clipping is never a surprise.
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .w(px(124.0))
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_xs()
                                    .child(
                                        div()
                                            .text_color(theme.text_faint)
                                            .child(self.t(Key::Master)),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme.text_muted)
                                            .child(format!("{master_gain_db:+.1} dB")),
                                    ),
                            )
                            .child(div().h(px(7.0)).mt_1().child(level_meter(
                                db_to_meter_position(master_db),
                                db_to_meter_position(master_db),
                                Axis::Horizontal,
                                theme.meter_color(master_db),
                                &theme,
                            ))),
                    ),
            )
    }

    /// A drag-and-scroll tempo readout.
    fn render_tempo_control(
        &self,
        bpm: f64,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = self.theme.clone();
        div()
            .id("tempo")
            .flex()
            .flex_col()
            .justify_center()
            .w(px(84.0))
            .px_2()
            .py_1()
            .rounded(Metrics::RADIUS_MD)
            .bg(theme.surface_sunken)
            .border_1()
            .border_color(theme.border_subtle)
            .cursor_pointer()
            .hover(|this| this.border_color(theme.border))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_faint)
                    .child(self.t(Key::Tempo)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text)
                    .child(format!("{bpm:.2}")),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                    let start_bpm = this.project().bpm();
                    this.begin_drag(Drag::Tempo {
                        start_bpm,
                        start_x: event.position.x,
                    });
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                let bpm = this.project().bpm() + f64::from(notches);
                this.session.set_bpm(bpm);
                cx.notify();
            }))
    }

    /// Shows `tab` in the bottom editor, or hides the panel when that tab is already showing.
    pub(crate) fn show_editor_tab(&mut self, tab: EditorTab) {
        if self.panels.editor_visible && self.editor == tab {
            self.panels.editor_visible = false;
        } else {
            self.editor = tab;
            self.panels.editor_visible = true;
        }
    }

    /// Turns looping on or off.
    pub(crate) fn toggle_loop(&mut self) {
        let enabled = self.project().loop_enabled;
        self.session.set_loop_enabled(!enabled);
    }

    /// Steps the editing grid to the next finer division, wrapping at the end.
    pub(crate) fn cycle_grid(&mut self) {
        let current = self.project().grid.raw();
        let index = GRID_CHOICES
            .iter()
            .position(|(_, ticks)| *ticks == current)
            .unwrap_or(2);
        let (_, ticks) = GRID_CHOICES[(index + 1) % GRID_CHOICES.len()];
        self.session.set_grid(Ticks(ticks));
    }
}

impl AurisApp {
    /// The grid division as it appears on the button.
    ///
    /// The fractions are notation rather than words, so only the fallback needs translating.
    fn grid_label(&self) -> &'static str {
        GRID_CHOICES
            .iter()
            .find(|(_, ticks)| *ticks == self.project().grid.raw())
            .map(|(label, _)| *label)
            .unwrap_or_else(|| self.t(Key::GridFree))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grid_choice_is_a_distinct_division() {
        let mut seen: Vec<i64> = Vec::new();
        for (label, ticks) in GRID_CHOICES {
            assert!(label.starts_with("1/"), "`{label}` is not a division");
            assert!(!seen.contains(&ticks), "`{label}` repeats a division");
            seen.push(ticks);
        }
        // Coarsest first, so cycling through them halves the division each time.
        assert!(seen.windows(2).all(|pair| pair[0] > pair[1]));
    }
}
