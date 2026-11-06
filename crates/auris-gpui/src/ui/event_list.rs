//! The piano roll's event list: the clip in hand as a table, one row per event.
//!
//! The grid is the picture of a clip and this is its text — where each event falls (bar, tick
//! within the bar, and the step from the one above), what it is, how long it is held and how
//! hard. A row is a note or a point on one of the clip's curves, which is everything a clip
//! sends an instrument. Every cell but the step's is the same field the roll drags, so a value
//! typed here and a value dragged there are the same edit; the commands are
//! [`Session::clip_events`] and [`Session::edit_clip_event`], and this module only turns their
//! rows into text and text back into an [`EventEdit`].
//!
//! Selecting a note's row selects that note in the roll and scrolls the grid to it, so the two
//! views are one selection seen twice. A double-click on a cell opens the sheet that edits it,
//! and Return there walks on to the same cell of the next row — the flow a list editor teaches.

use auris_i18n::controller::controller_name;
use auris_i18n::{Key, Language};
use auris_session::prelude::*;
use auris_session::{ClipEvent, ClipEventKind, EventEdit, EventRef, parse_root_note};

use gpui::{MouseButton, MouseDownEvent, Pixels, SharedString, div, prelude::*, px};

use crate::app::AurisApp;
use crate::ui::prompt::{Prompt, PromptTarget};
use crate::ui::timeline::pitch_name;

/// How wide the list is drawn.
///
/// Wide enough for the six columns at the interface's text size with the event's name in full,
/// and no wider: every pixel it takes is a pixel of grid the notes no longer have.
pub(crate) const EVENT_LIST_WIDTH: Pixels = px(340.0);

/// How tall one row is.
const ROW_HEIGHT: Pixels = px(18.0);

/// What the list remembers between frames.
#[derive(Default)]
pub(crate) struct EventListState {
    /// Where the list is scrolled to.
    pub scroll: gpui::UniformListScrollHandle,
    /// The row last clicked, when it is a curve point — a note's row is selected through the
    /// roll's own note selection, so the two never disagree about it.
    pub row: Option<EventRef>,
}

/// One column of the list.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum EventColumn {
    /// The bar the event falls in, from 1.
    Measure,
    /// Ticks from the start of that bar.
    Tick,
    /// Ticks after the event in the row above.
    Step,
    /// What the event is: a note's name, or which controller.
    Event,
    /// How long a note is held.
    Gate,
    /// A note's velocity, a controller's value, or the bend in semitones.
    Value,
}

impl EventColumn {
    /// Every column, left to right.
    pub const ALL: [EventColumn; 6] = [
        EventColumn::Measure,
        EventColumn::Tick,
        EventColumn::Step,
        EventColumn::Event,
        EventColumn::Gate,
        EventColumn::Value,
    ];

    /// The column's heading.
    pub fn label(self) -> Key {
        match self {
            EventColumn::Measure => Key::EventListMeasure,
            EventColumn::Tick => Key::EventListTick,
            EventColumn::Step => Key::EventListStep,
            EventColumn::Event => Key::EventListEvent,
            EventColumn::Gate => Key::EventListGate,
            EventColumn::Value => Key::EventListValue,
        }
    }

    /// How wide the column is drawn, or `None` for the one that takes what is left.
    fn width(self) -> Option<Pixels> {
        match self {
            EventColumn::Measure => Some(px(40.0)),
            EventColumn::Tick | EventColumn::Step | EventColumn::Gate => Some(px(46.0)),
            EventColumn::Event => None,
            EventColumn::Value => Some(px(58.0)),
        }
    }

    /// Whether a row of this kind has the column at all.
    ///
    /// A curve point has a place and a value and nothing else: no key to put it on and no
    /// length to hold it for.
    pub fn applies_to(self, event: EventRef) -> bool {
        match self {
            EventColumn::Event | EventColumn::Gate => matches!(event, EventRef::Note(_)),
            _ => true,
        }
    }

    /// The line under the sheet's field saying what it takes.
    pub fn hint(self, event: EventRef) -> Key {
        match self {
            EventColumn::Measure => Key::HintEventMeasure,
            EventColumn::Tick => Key::HintEventTick,
            EventColumn::Step => Key::HintEventStep,
            EventColumn::Event => Key::HintEventPitch,
            EventColumn::Gate => Key::HintEventGate,
            EventColumn::Value => match event {
                EventRef::Note(_) => Key::HintEventVelocity,
                EventRef::Curve {
                    curve: ClipCurve::Bend,
                    ..
                } => Key::HintEventBend,
                EventRef::Curve { .. } => Key::HintEventController,
            },
        }
    }
}

/// Where an event falls in the song, as the list reads it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Place {
    /// The bar, from 1.
    bar: u32,
    /// Ticks from the start of that bar.
    tick: i64,
}

fn place(signatures: &SignatureMap, at: Ticks) -> Place {
    Place {
        bar: signatures.bar_of(at),
        tick: (at - signatures.bar_floor(at)).raw(),
    }
}

/// What a row's event column says.
fn event_name(kind: ClipEventKind, language: Language) -> String {
    match kind {
        ClipEventKind::Note { pitch, .. } => pitch_name(pitch),
        ClipEventKind::Controller { number, .. } => match controller_name(number, language) {
            Some(name) => format!("CC#{number} {name}"),
            None => format!("CC#{number}"),
        },
        ClipEventKind::Bend { .. } => Key::CmdShowBendLane.get(language).to_string(),
    }
}

/// Every cell of one row, as text, in [`EventColumn::ALL`] order.
///
/// `previous` is where the row above falls, in song time, or the clip's start for the first
/// row: a step is always measured from something, and the clip's beginning is what the first
/// event is late by.
fn row_cells(
    event: &ClipEvent,
    clip_start: Ticks,
    previous: Ticks,
    signatures: &SignatureMap,
    language: Language,
) -> [String; 6] {
    let at = clip_start + event.at;
    let Place { bar, tick } = place(signatures, at);
    let (gate, value) = match event.kind {
        ClipEventKind::Note { gate, velocity, .. } => {
            (gate.raw().to_string(), velocity.to_string())
        }
        ClipEventKind::Controller { value, .. } => (String::new(), value.to_string()),
        ClipEventKind::Bend { semitones } => (String::new(), format!("{semitones:+.2}")),
    };
    [
        bar.to_string(),
        tick.to_string(),
        (at - previous).raw().to_string(),
        event_name(event.kind, language),
        gate,
        value,
    ]
}

/// What the sheet opens holding for one cell.
fn cell_text(
    events: &[ClipEvent],
    row: usize,
    column: EventColumn,
    clip_start: Ticks,
    signatures: &SignatureMap,
) -> String {
    let previous = row
        .checked_sub(1)
        .map_or(clip_start, |above| clip_start + events[above].at);
    let cells = row_cells(
        &events[row],
        clip_start,
        previous,
        signatures,
        Language::English,
    );
    let index = EventColumn::ALL
        .iter()
        .position(|each| *each == column)
        .unwrap_or(0);
    match (column, events[row].kind) {
        // The name for reading; a number is as good for typing and the parser takes either.
        (EventColumn::Event, ClipEventKind::Note { pitch, .. }) => pitch_name(pitch),
        _ => cells[index].clone(),
    }
}

/// Reads what was typed into one cell of the row at `row` as an edit, or `None` when it is not
/// something that cell can hold.
///
/// Out-of-range numbers are refused rather than clamped: the sheet says what the range is, and
/// a typo of 1270 for 127 is better caught than quietly turned into the loudest note there is.
pub(crate) fn parse_cell(
    events: &[ClipEvent],
    row: usize,
    column: EventColumn,
    text: &str,
    clip_start: Ticks,
    signatures: &SignatureMap,
) -> Option<EventEdit> {
    let event = events.get(row)?;
    if !column.applies_to(event.event) {
        return None;
    }
    let text = text.trim();
    let at = clip_start + event.at;
    let whole = || text.parse::<i64>().ok();
    Some(match column {
        EventColumn::Measure => {
            let bar = text.parse::<u32>().ok().filter(|bar| *bar >= 1)?;
            let into = at - signatures.bar_floor(at);
            EventEdit::Position(signatures.bar_start(bar) + into - clip_start)
        }
        EventColumn::Tick => {
            let tick = whole().filter(|tick| *tick >= 0)?;
            EventEdit::Position(signatures.bar_floor(at) + Ticks(tick) - clip_start)
        }
        EventColumn::Step => {
            let step = whole().filter(|step| *step >= 0)?;
            let previous = row
                .checked_sub(1)
                .map_or(Ticks::ZERO, |above| events[above].at);
            EventEdit::Position(previous + Ticks(step))
        }
        EventColumn::Event => EventEdit::Pitch(parse_root_note(text)?),
        EventColumn::Gate => EventEdit::Gate(Ticks(whole().filter(|gate| *gate >= 1)?)),
        EventColumn::Value => {
            let value = text.parse::<f32>().ok().filter(|value| value.is_finite())?;
            let (low, high) = match event.kind {
                ClipEventKind::Note { .. } => (1.0, 127.0),
                ClipEventKind::Controller { .. } => (0.0, 127.0),
                ClipEventKind::Bend { .. } => (-BEND_LIMIT, BEND_LIMIT),
            };
            EventEdit::Value((low..=high).contains(&value).then_some(value)?)
        }
    })
}

/// A row ready to draw.
struct DrawnRow {
    event: EventRef,
    cells: [String; 6],
    selected: bool,
    beyond_clip: bool,
}

impl AurisApp {
    /// Shows or hides the roll's event list, and remembers it.
    pub(crate) fn toggle_event_list(&mut self) {
        self.panels.event_list = !self.panels.event_list;
        self.remember_layout();
    }

    /// The list's rows for `clip`, in the session's order.
    fn event_rows(&self, clip: ClipId) -> Vec<ClipEvent> {
        self.session.clip_events(clip).unwrap_or_default()
    }

    /// Opens the sheet that edits one cell.
    pub(crate) fn open_event_prompt(&mut self, clip: ClipId, event: EventRef, column: EventColumn) {
        if !column.applies_to(event) {
            return;
        }
        let events = self.event_rows(clip);
        let Some(row) = events.iter().position(|each| each.event == event) else {
            return;
        };
        let Some(clip_start) = self.session.midi_clip(clip).map(|target| target.start) else {
            return;
        };
        let text = cell_text(&events, row, column, clip_start, &self.project().signatures);
        self.select_event(event);
        let title = format!("{} — {}", self.t(Key::EventList), self.t(column.label()));
        self.open_prompt(Prompt::new(
            title,
            PromptTarget::Event {
                clip,
                event,
                column,
            },
            text,
        ));
    }

    /// Applies what was typed into a cell, then walks the sheet on to the same cell of the next
    /// row that has one.
    pub(crate) fn commit_event_prompt(
        &mut self,
        clip: ClipId,
        event: EventRef,
        column: EventColumn,
        text: &str,
    ) -> Result<(), String> {
        let events = self.event_rows(clip);
        let row = events.iter().position(|each| each.event == event);
        let clip_start = self.session.midi_clip(clip).map(|target| target.start);
        let (Some(row), Some(clip_start)) = (row, clip_start) else {
            return Err(self.t(Key::EventListEmpty).to_string());
        };
        let Some(edit) = parse_cell(
            &events,
            row,
            column,
            text,
            clip_start,
            &self.project().signatures,
        ) else {
            return Err(self.t(column.hint(event)).to_string());
        };
        let landed = self
            .session
            .edit_clip_event(clip, event, edit)
            .map_err(|error| error.to_string())?;
        self.select_event(landed);
        // The next row along in the list as it stands now, which a moved event has rejoined
        // wherever it now falls.
        let events = self.event_rows(clip);
        let next = events
            .iter()
            .position(|each| each.event == landed)
            .and_then(|at| {
                events[at + 1..]
                    .iter()
                    .find(|each| column.applies_to(each.event))
            })
            .map(|each| each.event);
        if let Some(next) = next {
            self.open_event_prompt(clip, next, column);
        }
        Ok(())
    }

    /// Makes `event` the list's selected row, and the roll's selected note when it is one.
    fn select_event(&mut self, event: EventRef) {
        self.event_list.row = Some(event);
        self.selected_notes.clear();
        if let EventRef::Note(index) = event {
            self.selected_notes.insert(index);
        }
    }

    /// Scrolls the grid so the row's event is in view: its time, and a note's pitch.
    fn reveal_event(&mut self, clip: ClipId, event: &ClipEvent) {
        let Some(clip_start) = self.session.midi_clip(clip).map(|target| target.start) else {
            return;
        };
        let Some(bounds) = self.canvas.roll.get() else {
            return;
        };
        self.timeline
            .scroll_to_reveal(clip_start + event.at, bounds.size.width);
        if let ClipEventKind::Note { pitch, .. } = event.kind {
            let top = self.pitch.y_to_pitch(px(0.0));
            let bottom = self.pitch.y_to_pitch(bounds.size.height);
            if pitch > top || pitch < bottom {
                self.pitch.center_on(pitch, bounds.size.height);
            }
        }
    }

    /// A press on one cell: select the row, or on a second click edit the cell.
    fn press_event_cell(
        &mut self,
        clip: ClipId,
        event: EventRef,
        column: EventColumn,
        mouse: &MouseDownEvent,
    ) {
        if mouse.click_count >= 2 {
            self.open_event_prompt(clip, event, column);
            return;
        }
        let extend = mouse.modifiers.shift || mouse.modifiers.secondary();
        match (event, extend) {
            (EventRef::Note(index), true) => {
                self.event_list.row = Some(event);
                if !self.selected_notes.remove(&index) {
                    self.selected_notes.insert(index);
                }
            }
            _ => self.select_event(event),
        }
        if let Some(row) = self
            .event_rows(clip)
            .into_iter()
            .find(|each| each.event == event)
        {
            self.reveal_event(clip, &row);
        }
    }

    /// The list itself: a heading row and the events of `clip` under it.
    pub(crate) fn render_event_list(
        &mut self,
        clip: ClipId,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let language = self.language();
        let events = self.event_rows(clip);
        let (clip_start, clip_length) = self
            .session
            .midi_clip(clip)
            .map(|target| (target.start, target.length))
            .unwrap_or((Ticks::ZERO, Ticks::ZERO));
        let signatures = self.project().signatures.clone();
        let mut previous = clip_start;
        let rows: Vec<DrawnRow> = events
            .iter()
            .map(|event| {
                let cells = row_cells(event, clip_start, previous, &signatures, language);
                previous = clip_start + event.at;
                let selected = match event.event {
                    EventRef::Note(index) => self.selected_notes.contains(&index),
                    curve => self.event_list.row == Some(curve),
                };
                DrawnRow {
                    event: event.event,
                    cells,
                    selected,
                    beyond_clip: event.at >= clip_length,
                }
            })
            .collect();
        let rows = std::sync::Arc::new(rows);

        let cell = |column: EventColumn, text: SharedString| {
            let base = div()
                .h_full()
                .px_1()
                .flex()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap();
            let base = match column.width() {
                Some(width) => base.w(width).flex_shrink_0().justify_end(),
                None => base.flex_1().min_w_0(),
            };
            base.child(text)
        };

        let heading = div()
            .flex()
            .flex_shrink_0()
            .h(ROW_HEIGHT + px(4.0))
            .bg(theme.surface_raised)
            .border_b_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.text_muted)
            .children(
                EventColumn::ALL
                    .into_iter()
                    .map(|column| cell(column, self.t(column.label()).into())),
            );

        let body = if rows.is_empty() {
            div()
                .flex_1()
                .p_2()
                .text_xs()
                .text_color(theme.text_faint)
                .child(self.t(Key::EventListEmpty))
                .into_any_element()
        } else {
            let count = rows.len();
            let theme = theme.clone();
            gpui::uniform_list(
                "event-list",
                count,
                cx.processor(move |_app, range: std::ops::Range<usize>, _window, cx| {
                    range
                        .map(|index| {
                            let row = &rows[index];
                            let event = row.event;
                            let colour = if row.selected {
                                theme.text_on_accent
                            } else if row.beyond_clip {
                                theme.text_faint
                            } else {
                                match event {
                                    EventRef::Note(_) => theme.text,
                                    EventRef::Curve { .. } => theme.accent_text,
                                }
                            };
                            div()
                                .id(("event-row", index))
                                .flex()
                                // Across the whole list, so the columns line up with the heading.
                                .w_full()
                                .h(ROW_HEIGHT)
                                .text_xs()
                                .text_color(colour)
                                .when(row.selected, |this| this.bg(theme.accent))
                                .when(!row.selected, |this| {
                                    this.hover(|style| style.bg(theme.surface_hover))
                                })
                                .children(EventColumn::ALL.into_iter().zip(row.cells.iter()).map(
                                    |(column, text)| {
                                        cell(column, text.clone().into())
                                            .debug_selector(move || {
                                                format!("event-cell-{index}-{column:?}")
                                            })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    move |app, mouse: &MouseDownEvent, _, cx| {
                                                        app.press_event_cell(
                                                            clip, event, column, mouse,
                                                        );
                                                        cx.notify();
                                                    },
                                                ),
                                            )
                                    },
                                ))
                        })
                        .collect()
                }),
            )
            .track_scroll(self.event_list.scroll.clone())
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .id("event-list-panel")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(EVENT_LIST_WIDTH)
            .h_full()
            .bg(theme.surface)
            .border_r_1()
            .border_color(theme.border)
            .child(heading)
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{double_click, paint, with_a_clip};
    use gpui::TestAppContext;

    fn cell_at(
        cx: &mut gpui::VisualTestContext,
        row: usize,
        column: EventColumn,
    ) -> gpui::Point<Pixels> {
        let selector: &'static str =
            Box::leak(format!("event-cell-{row}-{column:?}").into_boxed_str());
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("no cell `{selector}` was drawn"))
            .center()
    }

    #[gpui::test]
    fn the_list_selects_with_the_roll_and_return_walks_down_a_column(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.session
                .add_note(clip, Note::new(60, Ticks::ZERO, Ticks(480)))
                .unwrap();
            this.session
                .add_note(clip, Note::new(64, Ticks(960), Ticks(480)))
                .unwrap();
            this.session
                .set_curve_point(clip, ClipCurve::MODULATION, Ticks(480), 0.5);
            this.open_clip_in_editor(clip);
            this.panels.event_list = true;
        });
        paint(&app, cx);

        // The second note is the third row: the wheel point falls between the two notes.
        let at = cell_at(cx, 2, EventColumn::Event);
        cx.simulate_click(at, gpui::Modifiers::none());
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.selected_notes.iter().copied().collect::<Vec<_>>(), [1]);
        });

        let at = cell_at(cx, 0, EventColumn::Value);
        double_click(cx, at);
        paint(&app, cx);
        cx.simulate_input("90");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        // Return walked on to the wheel point's value.
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.prompt.as_ref().and_then(|prompt| prompt.target()),
                Some(PromptTarget::Event {
                    clip,
                    event: EventRef::Curve {
                        curve: ClipCurve::MODULATION,
                        at: Ticks(480)
                    },
                    column: EventColumn::Value,
                })
            );
        });
        cx.simulate_input("127");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        cx.simulate_keystrokes("escape");
        paint(&app, cx);

        app.update(cx, |this, _| {
            let target = this.session.midi_clip(clip).unwrap();
            assert_eq!((target.notes[0].velocity * 127.0).round() as u8, 90);
            assert_eq!(target.curve(ClipCurve::MODULATION)[0].value, 1.0);
            assert!(this.prompt.is_none());
            // Each Return was one step.
            this.session.undo();
            let target = this.session.midi_clip(clip).unwrap();
            assert_eq!(target.curve(ClipCurve::MODULATION)[0].value, 0.5);
            assert_eq!((target.notes[0].velocity * 127.0).round() as u8, 90);
        });
    }

    #[gpui::test]
    fn a_column_a_row_does_not_have_opens_no_sheet(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.session
                .set_curve_point(clip, ClipCurve::Bend, Ticks::ZERO, 1.0);
            this.open_clip_in_editor(clip);
            this.panels.event_list = true;
        });
        paint(&app, cx);
        let at = cell_at(cx, 0, EventColumn::Gate);
        double_click(cx, at);
        paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.prompt.is_none()));
    }

    const BAR: i64 = 3840;

    fn note(index: usize, at: i64, pitch: u8) -> ClipEvent {
        ClipEvent {
            at: Ticks(at),
            event: EventRef::Note(index),
            kind: ClipEventKind::Note {
                pitch,
                velocity: 100,
                gate: Ticks(480),
            },
        }
    }

    fn wheel(at: i64) -> ClipEvent {
        ClipEvent {
            at: Ticks(at),
            event: EventRef::Curve {
                curve: ClipCurve::MODULATION,
                at: Ticks(at),
            },
            kind: ClipEventKind::Controller {
                number: 1,
                value: 64,
            },
        }
    }

    fn four_four() -> SignatureMap {
        SignatureMap::constant(TimeSignature::new(4, 4))
    }

    #[test]
    fn a_row_reads_bar_tick_step_and_what_it_is_in_song_time() {
        // A clip starting on bar 2, its event a beat and a half in.
        let event = note(0, 1440, 60);
        let cells = row_cells(
            &event,
            Ticks(BAR),
            Ticks(BAR),
            &four_four(),
            Language::English,
        );
        assert_eq!(
            cells,
            ["2", "1440", "1440", "C4", "480", "100"].map(String::from)
        );

        let cells = row_cells(
            &wheel(0),
            Ticks::ZERO,
            Ticks::ZERO,
            &four_four(),
            Language::English,
        );
        assert_eq!(cells[3], "CC#1 Modulation");
        assert_eq!(cells[4], "", "a controller has no gate");
    }

    #[test]
    fn the_bend_reads_in_signed_semitones() {
        let bend = ClipEvent {
            at: Ticks::ZERO,
            event: EventRef::Curve {
                curve: ClipCurve::Bend,
                at: Ticks::ZERO,
            },
            kind: ClipEventKind::Bend { semitones: 2.0 },
        };
        let cells = row_cells(
            &bend,
            Ticks::ZERO,
            Ticks::ZERO,
            &four_four(),
            Language::Japanese,
        );
        assert_eq!(cells[3], "ピッチベンド");
        assert_eq!(cells[5], "+2.00");
    }

    #[test]
    fn typed_places_land_in_clip_time() {
        let events = [note(0, 0, 60), note(1, 1440, 64)];
        let start = Ticks(BAR);
        let parse = |column, text| parse_cell(&events, 1, column, text, start, &four_four());
        // Bar 3 keeps the tick within the bar: one bar later than it was.
        assert_eq!(
            parse(EventColumn::Measure, "3"),
            Some(EventEdit::Position(Ticks(BAR + 1440)))
        );
        assert_eq!(
            parse(EventColumn::Tick, "960"),
            Some(EventEdit::Position(Ticks(960)))
        );
        // A step is measured from the row above.
        assert_eq!(
            parse(EventColumn::Step, "240"),
            Some(EventEdit::Position(Ticks(240)))
        );
        assert_eq!(parse(EventColumn::Measure, "0"), None);
        assert_eq!(parse(EventColumn::Tick, "-1"), None);
    }

    #[test]
    fn typed_values_are_read_in_the_rows_own_units_and_refused_out_of_range() {
        let events = [note(0, 0, 60), wheel(0)];
        let parse =
            |row, column, text| parse_cell(&events, row, column, text, Ticks::ZERO, &four_four());
        assert_eq!(
            parse(0, EventColumn::Event, "F#3"),
            Some(EventEdit::Pitch(54))
        );
        assert_eq!(
            parse(0, EventColumn::Event, "72"),
            Some(EventEdit::Pitch(72))
        );
        assert_eq!(
            parse(0, EventColumn::Gate, "120"),
            Some(EventEdit::Gate(Ticks(120)))
        );
        assert_eq!(parse(0, EventColumn::Gate, "0"), None);
        assert_eq!(
            parse(0, EventColumn::Value, "127"),
            Some(EventEdit::Value(127.0))
        );
        assert_eq!(
            parse(0, EventColumn::Value, "0"),
            None,
            "velocity 0 is note-off"
        );
        assert_eq!(
            parse(1, EventColumn::Value, "0"),
            Some(EventEdit::Value(0.0))
        );
        assert_eq!(parse(1, EventColumn::Value, "128"), None);
        assert_eq!(
            parse(1, EventColumn::Gate, "120"),
            None,
            "no gate on a controller"
        );
        assert_eq!(parse(1, EventColumn::Event, "C4"), None);
    }
}
