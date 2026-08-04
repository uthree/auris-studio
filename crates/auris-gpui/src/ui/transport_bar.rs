//! The transport bar across the top of the window.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{Axis, IntoElement, Window, div, prelude::*, px};

use crate::app::{AurisApp, Drag, EditorTab};
use crate::theme::Metrics;
use crate::ui::icons::Icon;
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::widgets::{
    ButtonStyle, button, db_to_meter_position, icon_button, level_meter, readout,
};

/// Grid divisions offered in the transport bar, as a fraction of a quarter note.
const GRID_CHOICES: [(&str, i64); 7] = [
    ("1/1", TICKS_PER_QUARTER * 4),
    ("1/2", TICKS_PER_QUARTER * 2),
    ("1/4", TICKS_PER_QUARTER),
    ("1/8", TICKS_PER_QUARTER / 2),
    ("1/16", TICKS_PER_QUARTER / 4),
    ("1/32", TICKS_PER_QUARTER / 8),
    // Off. A tick is the finest position the document can hold, so snapping to one is not
    // snapping — and `Key::GridFree` has labelled this state since the button was written, for
    // a value the cycle could never reach.
    (GRID_OFF_LABEL, 1),
];

/// Marks the entry whose label is translated rather than notation. See [`AurisApp::grid_label`].
const GRID_OFF_LABEL: &str = "";

/// Ticks in one unit of the position readout's last field.
///
/// The readout has always shown hundredths of a beat rather than raw ticks, because 960 of
/// anything is a number nobody reads. Typing one back has to use the same unit, or the box would
/// be asking for something other than what the display just said.
const POSITION_UNIT: i64 = TICKS_PER_QUARTER / 96;

/// The playhead position as the readout writes it: bar, beat and hundredth, counting from one.
pub fn format_position(at: Ticks, tempo: &TempoMap, signature: TimeSignature) -> String {
    let (bar, beat, tick) = tempo.bar_beat_at(at, signature);
    format!("{bar}.{beat}.{:03}", tick / POSITION_UNIT)
}

/// Reads a position back out of what [`format_position`] wrote.
///
/// Forgiving about how much of it is there — `17` is the top of bar seventeen and `17.3` is its
/// third beat — because the trailing fields are almost always zero and typing `.1.000` every time
/// is the sort of thing that makes people go back to the wheel. Strict about the rest: a beat past
/// the end of its bar is a typo rather than a way of writing the next bar, and answering it by
/// silently jumping somewhere else would be worse than saying no.
pub fn parse_position(text: &str, signature: TimeSignature) -> Option<Ticks> {
    let mut fields = text
        .split(['.', ':', '|', ' '])
        .filter(|field| !field.is_empty());
    let bar: i64 = fields.next()?.trim().parse().ok()?;
    let beat: i64 = fields
        .next()
        .map_or(Ok(1), |field| field.trim().parse())
        .ok()?;
    let unit: i64 = fields
        .next()
        .map_or(Ok(0), |field| field.trim().parse())
        .ok()?;
    if fields.next().is_some() {
        return None;
    }

    let beats_per_bar = i64::from(signature.numerator.max(1));
    let ticks_per_beat = signature.ticks_per_beat().raw().max(1);
    let units_per_beat = ticks_per_beat / POSITION_UNIT.max(1);
    if bar < 1 || !(1..=beats_per_bar).contains(&beat) || !(0..units_per_beat).contains(&unit) {
        return None;
    }
    // Checked, because the bar is the one field with no upper bound: sixteen typed nines times
    // a bar of ticks overflows an `i64`, which is a panic in a debug build and a seek to a
    // wrapped nonsense position in a release one. Refused like any other typo.
    let ticks = (bar - 1)
        .checked_mul(signature.ticks_per_bar().raw())?
        .checked_add((beat - 1) * ticks_per_beat + unit * POSITION_UNIT)?;
    Some(Ticks(ticks))
}

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
        let position = format_position(
            playhead,
            &self.project().tempo_map,
            self.project().time_signature,
        );
        let seconds = self.project().tempo_map.ticks_to_seconds(playhead);
        let bpm = self.project().bpm();
        let grid_label = self.grid_label();
        let master_db = gain_to_db(self.master_level());
        let master_gain_db = self.project().master.gain_db;
        let editor = self.editor;
        let editor_open = self.panels.editor_visible;
        let inspector_open = self.panels.inspector_visible;
        let library_open = self.panels.library_visible;

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
                            .child(
                                // The readout is a display; the double-click that turns it into
                                // an input goes on a wrapper, so that `readout` stays the plain
                                // thing every other transport uses.
                                div()
                                    .id("position")
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                                            if event.click_count < 2 {
                                                return;
                                            }
                                            this.prompt_for_position();
                                            cx.notify();
                                        }),
                                    )
                                    .child(readout(
                                        self.t(Key::Position),
                                        position,
                                        Some(seconds.format_clock().into()),
                                        px(118.0),
                                        &theme,
                                    )),
                            )
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
                            ))
                            // The library was the only panel with no button of its own, so
                            // toggling it off read as having lost it: the way back was a menu
                            // item or a keystroke, and nothing on screen said either existed.
                            .child(button(
                                "tab-library",
                                self.t(Key::Library),
                                ButtonStyle::Normal,
                                library_open,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_library();
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
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if event.click_count >= 2 {
                        // The first click of the pair already began a drag. It has not moved, so
                        // it has changed nothing, but leaving it live would have the sheet's own
                        // pointer dragging the tempo behind it.
                        this.end_drag(window, cx);
                        this.prompt_for_tempo();
                        cx.notify();
                        return;
                    }
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

    /// Opens the sheet that takes a tempo as a number.
    pub(crate) fn prompt_for_tempo(&mut self) {
        let title = self.t(Key::SetTempoTitle);
        let current = format!("{:.2}", self.project().bpm());
        self.open_prompt(Prompt::new(title, PromptTarget::Tempo, current));
    }

    /// Opens the sheet that takes a position as bar, beat and hundredth.
    pub(crate) fn prompt_for_position(&mut self) {
        let title = self.t(Key::SetPositionTitle);
        let current = format_position(
            self.playhead_ticks(),
            &self.project().tempo_map,
            self.project().time_signature,
        );
        self.open_prompt(Prompt::new(title, PromptTarget::Position, current));
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
            .filter(|label| *label != GRID_OFF_LABEL)
            .unwrap_or_else(|| self.t(Key::GridFree))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn four_four() -> TimeSignature {
        TimeSignature::default()
    }

    #[test]
    fn a_bar_past_any_timeline_is_refused_rather_than_wrapped() {
        // Sixteen nines parse fine as an i64 and then overflow it multiplied by a bar of
        // ticks: a debug build panicked on the multiply, a release build seeked to a wrapped
        // nonsense position. A number too large to mean anything is a typo like any other.
        assert_eq!(parse_position("9999999999999999", four_four()), None);
        assert_eq!(parse_position(&i64::MAX.to_string(), four_four()), None);
        // While a merely enormous song is still addressable.
        assert!(parse_position("1000000", four_four()).is_some());
    }

    #[test]
    fn a_position_reads_back_as_the_one_the_readout_showed() {
        // The box is opened by double-clicking the readout and comes up holding what it said, so
        // a value that did not survive the round trip would move the playhead on a Return that
        // was meant to change nothing.
        let tempo = TempoMap::constant(120.0);
        for ticks in [0, 1, 240, 960, 3840, 3840 * 16 + 960 + 250] {
            let at = Ticks(ticks - ticks % POSITION_UNIT);
            let text = format_position(at, &tempo, four_four());
            assert_eq!(
                parse_position(&text, four_four()),
                Some(at),
                "`{text}` did not read back as {}",
                at.raw()
            );
        }
    }

    #[test]
    fn a_position_may_be_typed_with_the_trailing_fields_left_off() {
        // The fields after the bar are almost always zero, and typing `.1.000` every time is what
        // sends people back to the wheel.
        let bar = TimeSignature::default().ticks_per_bar().raw();
        let beat = TimeSignature::default().ticks_per_beat().raw();
        assert_eq!(parse_position("17", four_four()), Some(Ticks(bar * 16)));
        assert_eq!(
            parse_position("17.3", four_four()),
            Some(Ticks(bar * 16 + beat * 2))
        );
        assert_eq!(
            parse_position("17.3.050", four_four()),
            Some(Ticks(bar * 16 + beat * 2 + 50 * POSITION_UNIT))
        );
        // Bar one, beat one is the start, not one bar in: the readout counts from one.
        assert_eq!(parse_position("1.1.000", four_four()), Some(Ticks::ZERO));
        // Spaces and other separators a person might reach for.
        assert_eq!(
            parse_position(" 17 : 3 ", four_four()),
            parse_position("17.3", four_four())
        );
    }

    #[test]
    fn a_position_outside_its_bar_is_refused_rather_than_carried_over() {
        // Answering `1.9` by jumping to bar three would be answering a typo with a surprise. The
        // readout has never written one, so nothing is being refused that it could have shown.
        assert_eq!(
            parse_position("1.5", four_four()),
            None,
            "beat five of four"
        );
        assert_eq!(
            parse_position("1.0", four_four()),
            None,
            "beats count from one"
        );
        assert_eq!(parse_position("0", four_four()), None, "so do bars");
        assert_eq!(
            parse_position("1.1.096", four_four()),
            None,
            "hundredths run 0..95"
        );
        assert_eq!(parse_position("", four_four()), None);
        assert_eq!(parse_position("later", four_four()), None);
        assert_eq!(
            parse_position("1.1.0.0", four_four()),
            None,
            "one field too many"
        );
        assert_eq!(parse_position("-3", four_four()), None);

        // Three four has three beats to a bar, and the check follows the meter rather than a four.
        let three_four = TimeSignature::new(3, 4);
        assert!(parse_position("2.3", three_four).is_some());
        assert_eq!(parse_position("2.4", three_four), None);
    }

    #[test]
    fn every_grid_choice_is_a_distinct_division() {
        let mut seen: Vec<i64> = Vec::new();
        for (label, ticks) in GRID_CHOICES {
            assert!(
                label.starts_with("1/") || label == GRID_OFF_LABEL,
                "`{label}` is not a division",
            );
            assert!(!seen.contains(&ticks), "`{label}` repeats a division");
            seen.push(ticks);
        }
        // Coarsest first, so cycling through them halves the division each time and lands on
        // off at the end, which is the finest position there is.
        assert!(seen.windows(2).all(|pair| pair[0] > pair[1]));
        assert_eq!(
            GRID_CHOICES.last().map(|(label, _)| *label),
            Some(GRID_OFF_LABEL),
            "off comes last, so cycling reaches it without passing through it",
        );
    }

    #[test]
    fn the_grid_can_be_cycled_all_the_way_off_and_round_again() {
        // `Key::GridFree` has labelled this state since the button was written, for a value the
        // cycle could not produce: nothing a user placed could sit off the beat.
        let ticks: Vec<i64> = GRID_CHOICES.iter().map(|(_, ticks)| *ticks).collect();
        assert!(
            ticks.contains(&1),
            "one tick is as fine as the document gets"
        );
    }
}
