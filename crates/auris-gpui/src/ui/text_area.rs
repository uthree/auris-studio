//! A multi-line editable text element: the caret, the selection, the IME's pre-edit, and the
//! registration that makes the platform type into it.
//!
//! [`crate::ui::prompt::editable_text`]'s taller sibling, for the fields that take verses
//! rather than names. The content's newlines are real — one line of the box is one line of the
//! text — and a click lands the caret on the character under it, which a one-line field never
//! needed because its Return committed before anyone wanted to go back.
//!
//! No soft wrap: a lyric's line is a phrase, and a break the layout invented would look exactly
//! like one the writer meant. A line longer than the box scrolls sideways under the caret
//! instead, the way the one-line field always has.

use std::cell::Cell;
use std::ops::Range;

use gpui::{
    Bounds, ElementInputHandler, IntoElement, Pixels, Point, SharedString, Window, canvas, point,
    prelude::*, px, size,
};

use crate::theme::Theme;
use crate::ui::paint;
use crate::ui::prompt::{FIELD_PADDING, TEXT_SIZE};

/// Height of one row of the area.
///
/// A little more air than the one-line field's box gives its single row, because rows stacked
/// tight read as a block rather than as lines.
pub(crate) const AREA_LINE_HEIGHT: Pixels = px(20.0);

/// Vertical inset between the area's border and its first row.
const AREA_PADDING_Y: Pixels = px(5.0);

/// How wide the stub marking a selected newline is drawn.
///
/// A selection running across lines covers characters that have no glyphs — the line breaks —
/// and without the stub the highlight would stop dead at each line's last character, reading
/// as several selections rather than one.
const NEWLINE_STUB: Pixels = px(6.0);

/// The height the area needs for `text`, clamped between `min_rows` and `max_rows`.
///
/// Counted from the content's newlines, because that is exactly what the paint will draw: this
/// element does not wrap. The minimum keeps an empty verse from being a one-row slit nobody
/// recognises as a place for several; the maximum keeps a long one from pushing everything
/// under it off the sheet.
pub(crate) fn area_height(text: &str, min_rows: usize, max_rows: usize) -> Pixels {
    let rows = (text.split('\n').count()).clamp(min_rows, max_rows);
    AREA_LINE_HEIGHT * rows as f32 + AREA_PADDING_Y * 2.0
}

thread_local! {
    /// Where the editable area was painted last, and how far it was scrolled sideways.
    ///
    /// A click on the area has to turn a window position into a byte offset, which takes the
    /// bounds and the scroll the paint used — and both are only known during paint, while the
    /// click arrives outside one. One cell serves the application for the caret's reason
    /// ([`crate::ui::text_field`]): one area at a time is being typed into.
    static AREA: Cell<Option<(Bounds<Pixels>, Pixels)>> = const { Cell::new(None) };
}

/// One logical line of `text`: its byte range, excluding the newline that ends it.
fn lines(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for segment in text.split('\n') {
        ranges.push(start..start + segment.len());
        start += segment.len() + 1;
    }
    ranges
}

/// The byte offset in `text` under a window position, for a click landing the caret.
///
/// Answers only while the area is on screen, from the bounds and scroll its last paint
/// recorded. A position outside the rows clamps to the nearest — above the first row is the
/// first row, past a line's end is that line's end — because a click near the box is aimed at
/// it.
pub(crate) fn area_offset_at(
    window: &mut Window,
    text: &str,
    position: Point<Pixels>,
) -> Option<usize> {
    let (bounds, scroll) = AREA.with(Cell::get)?;
    let lines = lines(text);
    let row = ((position.y - bounds.origin.y - AREA_PADDING_Y) / AREA_LINE_HEIGHT).floor();
    let row = (row.max(0.0) as usize).min(lines.len() - 1);
    let range = lines[row].clone();
    let x = position.x - bounds.origin.x - FIELD_PADDING + scroll;
    let segment: SharedString = text[range.clone()].to_string().into();
    let mut run = window.text_style().to_run(segment.len());
    run.color = gpui::black();
    let shaped = window
        .text_system()
        .shape_line(segment, TEXT_SIZE, &[run], None);
    Some(range.start + shaped.closest_index_for_x(x))
}

/// A multi-line editable text element, registered as the window's input target while painted.
pub(crate) fn editable_area<V: gpui::EntityInputHandler>(
    text: SharedString,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    focus: gpui::FocusHandle,
    view: gpui::Entity<V>,
    theme: Theme,
) -> impl IntoElement + use<V> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            // Registering the handler is only legal during paint, and only matters while this
            // element exists — which is exactly as long as the sheet holding it is open.
            window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
            paint_area(
                window,
                cx,
                bounds,
                &text,
                &selection,
                marked.clone(),
                &theme,
            );
        },
    )
    .size_full()
}

/// The offset the area has to keep on screen: the end of the IME's pre-edit while one is
/// composing — that is where the candidate is being chosen — and the caret otherwise.
fn watched_offset(selection: &Range<usize>, marked: Option<&Range<usize>>) -> usize {
    marked.map_or(selection.end, |marked| marked.end)
}

/// Draws the text line by line, with the selection, the caret and the IME's pre-edit underline
/// on whichever lines they cross.
fn paint_area(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    text: &SharedString,
    selection: &Range<usize>,
    marked: Option<Range<usize>>,
    theme: &Theme,
) {
    let lines = lines(text);
    // Measuring by shaping the text up to an offset keeps the caret on the same glyph edge the
    // text is actually drawn at, whatever the font does with the characters in between.
    let advance = |window: &mut Window, range: &Range<usize>, offset: usize| -> Pixels {
        let offset = offset.clamp(range.start, range.end);
        if offset == range.start {
            return px(0.0);
        }
        let head: SharedString = text[range.start..offset].to_string().into();
        let mut run = window.text_style().to_run(head.len());
        run.color = theme.text;
        window
            .text_system()
            .shape_line(head, TEXT_SIZE, &[run], None)
            .width
    };
    // The line an offset sits on: the one whose range holds it. An offset equal to a line's
    // end belongs to that line — the caret before the newline — because the next line's range
    // starts one byte later.
    let row_of = |offset: usize| -> usize {
        lines
            .iter()
            .position(|range| offset >= range.start && offset <= range.end)
            .unwrap_or(lines.len() - 1)
    };

    paint::clipped(window, bounds, |window| {
        // Sideways, under the caret, exactly as the one-line field scrolls — but applied to
        // every line at once, because lines that slid independently would shear the text.
        let watched = watched_offset(selection, marked.as_ref());
        let watched_row = row_of(watched);
        let caret_x = advance(window, &lines[watched_row], watched);
        let visible = bounds.size.width - FIELD_PADDING * 2.0;
        let scroll = (caret_x - visible).max(px(0.0));
        AREA.with(|area| area.set(Some((bounds, scroll))));

        let left = bounds.origin.x + FIELD_PADDING - scroll;
        let row_top = |row: usize| bounds.origin.y + AREA_PADDING_Y + AREA_LINE_HEIGHT * row as f32;
        let text_top = |row: usize| row_top(row) + (AREA_LINE_HEIGHT - TEXT_SIZE * 1.35) / 2.0;

        // Where the platform should put an IME's candidate list. Only knowable here, from the
        // shaped lines, and asked for outside a paint — see `text_field::set_caret_bounds`.
        crate::ui::text_field::set_caret_bounds(Bounds {
            origin: point(left + caret_x, row_top(watched_row)),
            size: size(px(1.0), AREA_LINE_HEIGHT),
        });

        for (row, range) in lines.iter().enumerate() {
            // The selection's stretch across this line, plus a stub for the newline when it
            // runs on — one highlight, not one per line.
            if !selection.is_empty() && selection.start <= range.end && selection.end >= range.start
            {
                let from = advance(window, range, selection.start);
                let to = advance(window, range, selection.end);
                let stub = if selection.end > range.end {
                    NEWLINE_STUB
                } else {
                    px(0.0)
                };
                paint::rect(
                    window,
                    Bounds {
                        origin: point(left + from, row_top(row) + px(1.0)),
                        size: size(to - from + stub, AREA_LINE_HEIGHT - px(2.0)),
                    },
                    Theme::translucent(theme.accent, 0.35),
                );
            }

            paint::label(
                window,
                cx,
                point(left, text_top(row)),
                text[range.clone()].to_string(),
                TEXT_SIZE,
                theme.text,
            );

            // The pre-edit is underlined rather than boxed, matching what every other
            // application on the platform does while an IME is composing.
            if let Some(marked) = &marked
                && marked.start <= range.end
                && marked.end >= range.start
            {
                let from = advance(window, range, marked.start);
                let to = advance(window, range, marked.end);
                paint::rect(
                    window,
                    Bounds {
                        origin: point(left + from, row_top(row) + AREA_LINE_HEIGHT - px(3.0)),
                        size: size(to - from, px(1.5)),
                    },
                    theme.accent,
                );
            }
        }

        if selection.is_empty() {
            let row = row_of(selection.start);
            let caret = advance(window, &lines[row], selection.start);
            paint::rect(
                window,
                Bounds {
                    origin: point(left + caret, row_top(row) + px(2.0)),
                    size: size(px(1.5), AREA_LINE_HEIGHT - px(4.0)),
                },
                theme.accent,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_carry_their_byte_ranges_and_an_empty_tail_is_a_line() {
        assert_eq!(lines("ab\ncd"), vec![0..2, 3..5]);
        // A trailing newline means the caret can sit on a line with nothing in it yet.
        assert_eq!(lines("ab\n"), vec![0..2, 3..3]);
        assert_eq!(lines(""), vec![0..0]);
        // Multi-byte: さ is three bytes, and the ranges stay on character boundaries.
        assert_eq!(lines("さ\nくら"), vec![0..3, 4..10]);
    }

    #[test]
    fn the_area_grows_with_its_lines_between_the_clamps() {
        assert_eq!(area_height("", 3, 10), area_height("a\nb\nc", 3, 10));
        assert!(area_height("a\nb\nc\nd", 3, 10) > area_height("", 3, 10));
        assert_eq!(
            area_height(&"x\n".repeat(40), 3, 10),
            area_height(&"x\n".repeat(9), 3, 10),
            "past the clamp the sheet scrolls instead"
        );
    }
}
