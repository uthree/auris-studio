//! The transport bar across the top of the window.

use auris_core::param::gain_to_db;
use auris_core::time::Ticks;
use auris_engine::EngineCommand;
use gpui::{Axis, IntoElement, Window, div, prelude::*, px};

use crate::app::{AurisApp, Drag, EditorTab};
use crate::theme::Metrics;
use crate::ui::widgets::{ButtonStyle, button, db_to_meter_position, glyph_button, level_meter};

/// Grid divisions offered in the transport bar, as a fraction of a quarter note.
const GRID_CHOICES: [(&str, i64); 6] = [
    ("1/1", auris_core::time::TICKS_PER_QUARTER * 4),
    ("1/2", auris_core::time::TICKS_PER_QUARTER * 2),
    ("1/4", auris_core::time::TICKS_PER_QUARTER),
    ("1/8", auris_core::time::TICKS_PER_QUARTER / 2),
    ("1/16", auris_core::time::TICKS_PER_QUARTER / 4),
    ("1/32", auris_core::time::TICKS_PER_QUARTER / 8),
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
        let looping = self.project.loop_enabled;
        let playhead = self.playhead_ticks();
        let (bar, beat, tick) = self
            .project
            .tempo_map
            .bar_beat_at(playhead, self.project.time_signature);
        let seconds = self.project.tempo_map.ticks_to_seconds(playhead);
        let bpm = self.project.bpm();
        let grid_label = grid_label(self.project.grid);
        let master_db = gain_to_db(self.master_level());
        let master_gain_db = self.project.master.gain_db;
        let editor = self.editor;

        div()
            .flex()
            .items_center()
            .gap_3()
            .h(Metrics::TRANSPORT_HEIGHT)
            .px_3()
            .bg(theme.surface_raised)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(glyph_button(
                        "rtz",
                        "\u{23ee}",
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.seek(Ticks::ZERO);
                            cx.notify();
                        }),
                    ))
                    .child(glyph_button(
                        "play",
                        if playing { "\u{23f8}" } else { "\u{25b6}" },
                        playing,
                        theme.playing,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_play();
                            cx.notify();
                        }),
                    ))
                    .child(glyph_button(
                        "stop",
                        "\u{23f9}",
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.send(EngineCommand::Stop);
                            this.seek(Ticks::ZERO);
                            cx.notify();
                        }),
                    ))
                    .child(glyph_button(
                        "loop",
                        "\u{1f501}",
                        looping,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_loop();
                            cx.notify();
                        }),
                    )),
            )
            // Musical position and wall-clock position, the two readouts every DAW shows.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .w(px(112.0))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.surface_sunken)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text)
                            .child(format!("{bar:>3}.{beat}.{:03}", tick / 10)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.text_muted)
                            .child(seconds.format_clock()),
                    ),
            )
            .child(self.render_tempo_control(bpm, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.text_muted).child("Grid"))
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
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(button(
                        "tab-roll",
                        "Piano Roll",
                        ButtonStyle::Normal,
                        editor == EditorTab::PianoRoll,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.editor = EditorTab::PianoRoll;
                            cx.notify();
                        }),
                    ))
                    .child(button(
                        "tab-mixer",
                        "Mixer",
                        ButtonStyle::Normal,
                        editor == EditorTab::Mixer,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.editor = EditorTab::Mixer;
                            cx.notify();
                        }),
                    )),
            )
            // Master fader and meter, always visible so clipping is never a surprise.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .w(px(120.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(format!("Master {master_gain_db:+.1} dB")),
                            )
                            .child(div().h(px(8.0)).mt_1().child(level_meter(
                                db_to_meter_position(master_db),
                                db_to_meter_position(master_db),
                                Axis::Horizontal,
                                theme.meter_color(master_db),
                                &theme,
                            ))),
                    )
                    .child(button(
                        "export",
                        "Export WAV",
                        ButtonStyle::Primary,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, window, cx| this.start_export(window, cx)),
                    )),
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
            .w(px(78.0))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme.surface_sunken)
            .cursor_pointer()
            .child(div().text_xs().text_color(theme.text_muted).child("Tempo"))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text)
                    .child(format!("{bpm:.2}")),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, _| {
                    this.edit("Change tempo");
                    this.drag = Some(Drag::Tempo {
                        start_bpm: this.project.bpm(),
                        start_x: event.position.x,
                    });
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let notches = f32::from(event.delta.pixel_delta(px(16.0)).y) / 16.0;
                this.set_bpm(this.project.bpm() + f64::from(notches));
                this.rebuild_graph();
                cx.notify();
            }))
    }

    /// Sets the project tempo.
    ///
    /// Deliberately does not republish the graph: a tempo drag calls this on every pointer
    /// move, and rebuilding would re-instantiate every plugin sixty times a second. The
    /// rebuild happens once when the drag ends.
    pub(crate) fn set_bpm(&mut self, bpm: f64) {
        self.project.set_bpm(bpm);
        self.dirty = true;
    }

    /// Turns looping on or off, seeding a default region when there is none.
    pub(crate) fn toggle_loop(&mut self) {
        self.project.loop_enabled = !self.project.loop_enabled;
        if self.project.loop_region.is_none() {
            let bars = self.project.time_signature.ticks_per_bar() * 2;
            self.project.loop_region = Some((Ticks::ZERO, bars));
        }
        self.push_loop_to_engine();
        self.dirty = true;
    }

    /// Forwards the loop region to the audio thread.
    pub(crate) fn push_loop_to_engine(&self) {
        let (start, end) = self
            .project
            .loop_region
            .unwrap_or((Ticks::ZERO, Ticks::ZERO));
        let rate = self.engine.sample_rate();
        self.send(EngineCommand::SetLoop {
            enabled: self.project.loop_enabled && end > start,
            start: self.project.tempo_map.ticks_to_samples(start, rate).raw(),
            end: self.project.tempo_map.ticks_to_samples(end, rate).raw(),
        });
    }

    /// Steps the editing grid to the next finer division, wrapping at the end.
    pub(crate) fn cycle_grid(&mut self) {
        let current = self.project.grid.raw();
        let index = GRID_CHOICES
            .iter()
            .position(|(_, ticks)| *ticks == current)
            .unwrap_or(2);
        let (_, ticks) = GRID_CHOICES[(index + 1) % GRID_CHOICES.len()];
        self.project.grid = Ticks(ticks);
    }
}

fn grid_label(grid: Ticks) -> &'static str {
    GRID_CHOICES
        .iter()
        .find(|(_, ticks)| *ticks == grid.raw())
        .map(|(label, _)| *label)
        .unwrap_or("free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grid_choice_has_a_label() {
        for (label, ticks) in GRID_CHOICES {
            assert_eq!(grid_label(Ticks(ticks)), label);
        }
    }

    #[test]
    fn an_unknown_grid_reads_as_free() {
        assert_eq!(grid_label(Ticks(7)), "free");
    }
}
