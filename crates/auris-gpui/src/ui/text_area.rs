//! A multi-line editable text element: the caret, the selection, the IME's pre-edit, and the
//! registration that makes the platform type into it.
//!
//! [`crate::ui::prompt::editable_text`]'s taller sibling, for the fields that take verses
//! rather than names. The content's newlines are real — one line of the box is one line of the
//! text — and a click lands the caret on the character under it, which a one-line field never
//! needed because its Return committed before anyone wanted to go back.
//!
//! The lyric path does not soft-wrap: a lyric's line is a phrase, and a break the layout invented
//! would look exactly like one the writer meant. [`editable_wrapped_area`] is the prose variant
//! used by the Agent composer, where a long Japanese sentence should follow the panel width.

use std::cell::Cell;
use std::ops::Range;

use gpui::{
    Bounds, ElementInputHandler, IntoElement, Pixels, Point, SharedString, TextAlign, Window,
    WrappedLine, canvas, point, prelude::*, px, size,
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
    // The caller draws a one-pixel border on each side. Reserve it outside the
    // content height, otherwise two requested rows fit only one complete row.
    AREA_LINE_HEIGHT * rows as f32 + AREA_PADDING_Y * 2.0 + px(2.0)
}

thread_local! {
    /// Where the editable area was painted last, and how far it was scrolled on each axis.
    ///
    /// A click on the area has to turn a window position into a byte offset, which takes the
    /// bounds and the scroll the paint used — and both are only known during paint, while the
    /// click arrives outside one. One cell serves the application for the caret's reason
    /// ([`crate::ui::text_field`]): one area at a time is being typed into.
    static AREA: Cell<Option<(Bounds<Pixels>, Pixels, Pixels)>> = const { Cell::new(None) };

    /// Geometry of the soft-wrapped message editor painted most recently.
    ///
    /// Lyrics use [`AREA`] because authored line breaks are musical structure. Agent messages
    /// use this second cell because prose must wrap at the panel edge. Keeping the two apart also
    /// means clicking one editor can never reuse the other editor's scrolling coordinates.
    static WRAPPED_AREA: Cell<Option<(Bounds<Pixels>, Pixels, usize)>> = const { Cell::new(None) };
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
    let (bounds, scroll_x, scroll_y) = AREA.with(Cell::get)?;
    let lines = lines(text);
    let row =
        ((position.y - bounds.origin.y - AREA_PADDING_Y + scroll_y) / AREA_LINE_HEIGHT).floor();
    let row = (row.max(0.0) as usize).min(lines.len() - 1);
    let range = lines[row].clone();
    let x = position.x - bounds.origin.x - FIELD_PADDING + scroll_x;
    let segment: SharedString = text[range.clone()].to_string().into();
    let mut run = window.text_style().to_run(segment.len());
    run.color = gpui::black();
    let shaped = window
        .text_system()
        .shape_line(segment, TEXT_SIZE, &[run], None);
    Some(range.start + shaped.closest_index_for_x(x))
}

/// The byte offset under a point in the soft-wrapped message editor.
///
/// The wrapped layout is recreated from the last painted bounds. Text shaping is cached by gpui,
/// so this stays identical to the pixels under the pointer without retaining a frame-owned layout
/// past paint.
pub(crate) fn wrapped_area_offset_at(
    window: &mut Window,
    text: &str,
    position: Point<Pixels>,
) -> Option<usize> {
    let (bounds, scroll_y, _) = WRAPPED_AREA.with(Cell::get)?;
    let width = (bounds.size.width - FIELD_PADDING * 2.0).max(px(1.0));
    let shared: SharedString = text.to_string().into();
    let run = window.text_style().to_run(shared.len());
    let shaped = window
        .text_system()
        .shape_text(shared, TEXT_SIZE, &[run], Some(width), None)
        .ok()?;
    let ranges = lines(text);
    let x = (position.x - bounds.origin.x - FIELD_PADDING)
        .max(px(0.0))
        .min(width);
    let y = (position.y - bounds.origin.y - AREA_PADDING_Y + scroll_y).max(px(0.0));
    let mut row_top = px(0.0);
    for (line, range) in shaped.iter().zip(ranges) {
        let height = line.size(AREA_LINE_HEIGHT).height;
        if y < row_top + height {
            let within = point(x, y - row_top);
            let local = line
                .closest_index_for_position(within, AREA_LINE_HEIGHT)
                .unwrap_or_else(|index| index)
                .min(range.len());
            return Some(range.start + local);
        }
        row_top += height;
    }
    Some(text.len())
}

/// How wide the margin the per-line annotations sit in is, when there are any.
///
/// Room for a two-digit count and a breath of air; the text scrolls sideways before it runs
/// under the numbers, exactly as it scrolls before running off the edge.
const ANNOTATION_GUTTER: Pixels = px(30.0);

/// A multi-line editable text element, registered as the window's input target while painted.
///
/// `annotations` is one short label per line — a note count, in the lyrics boxes — drawn
/// faint against the right edge, row for row with the text. Pass an empty list for a plain
/// area; rows past the list's end simply have no label.
pub(crate) fn editable_area<V: gpui::EntityInputHandler>(
    text: SharedString,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    annotations: Vec<SharedString>,
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
                &annotations,
                &theme,
            );
        },
    )
    .size_full()
}

/// A soft-wrapped multi-line editor for ordinary prose.
///
/// The lyric editor above deliberately scrolls long authored lines sideways because its line
/// breaks carry musical meaning. A message has no such contract: wrapping at the available width
/// keeps Japanese text readable in a narrow Agent panel. The caller supplies the stable focus
/// handle so this canvas is also a genuine platform text-input target and tab stop.
pub(crate) fn editable_wrapped_area<V: gpui::EntityInputHandler>(
    text: SharedString,
    selection: Range<usize>,
    marked: Option<Range<usize>>,
    focused: bool,
    focus: gpui::FocusHandle,
    view: gpui::Entity<V>,
    theme: Theme,
) -> impl IntoElement + use<V> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
            paint_wrapped_area(
                window,
                cx,
                bounds,
                &text,
                &selection,
                marked.clone(),
                focused,
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

/// First row drawn when `watched` must remain inside a window of `visible` rows.
fn first_visible_row(watched: usize, visible: usize) -> usize {
    watched.saturating_add(1).saturating_sub(visible.max(1))
}

/// Draws the text line by line, with the selection, the caret and the IME's pre-edit underline
/// on whichever lines they cross.
#[expect(
    clippy::too_many_arguments,
    reason = "one canvas, one bundle of what it draws"
)]
fn paint_area(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    text: &SharedString,
    selection: &Range<usize>,
    marked: Option<Range<usize>>,
    annotations: &[SharedString],
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
        let gutter = match annotations.is_empty() {
            true => px(0.0),
            false => ANNOTATION_GUTTER,
        };
        let visible = bounds.size.width - FIELD_PADDING * 2.0 - gutter;
        let scroll = (caret_x - visible).max(px(0.0));
        let visible_rows = ((bounds.size.height - AREA_PADDING_Y * 2.0) / AREA_LINE_HEIGHT)
            .floor()
            .max(1.0) as usize;
        let scroll_y = AREA_LINE_HEIGHT * first_visible_row(watched_row, visible_rows) as f32;
        AREA.with(|area| area.set(Some((bounds, scroll, scroll_y))));

        let left = bounds.origin.x + FIELD_PADDING - scroll;
        let row_top = |row: usize| {
            bounds.origin.y + AREA_PADDING_Y + AREA_LINE_HEIGHT * row as f32 - scroll_y
        };
        let text_top = |row: usize| row_top(row) + (AREA_LINE_HEIGHT - TEXT_SIZE * 1.35) / 2.0;

        // Where the platform should put an IME's candidate list. Only knowable here, from the
        // shaped lines, and asked for outside a paint — see `text_field::set_caret_bounds`.
        crate::ui::text_field::set_caret_bounds(Bounds {
            origin: point(left + caret_x, row_top(watched_row)),
            size: size(px(1.0), AREA_LINE_HEIGHT),
        });

        for (row, range) in lines.iter().enumerate() {
            let top = row_top(row);
            if top + AREA_LINE_HEIGHT <= bounds.origin.y
                || top >= bounds.origin.y + bounds.size.height
            {
                continue;
            }
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

            // The margin note for this row — a note count, in the lyrics boxes — pinned to
            // the right edge, unmoved by the sideways scroll: it annotates the line, not a
            // place in it.
            if let Some(label) = annotations.get(row).filter(|label| !label.is_empty()) {
                paint::label_right(
                    window,
                    cx,
                    point(
                        bounds.origin.x + bounds.size.width - FIELD_PADDING,
                        text_top(row) + px(1.0),
                    ),
                    label.clone(),
                    px(10.0),
                    theme.text_faint,
                );
            }

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

/// The wrapped row and x coordinate of a byte offset within one logical line.
fn wrapped_position(line: &WrappedLine, offset: usize) -> Point<Pixels> {
    line.position_for_index(offset.min(line.len()), AREA_LINE_HEIGHT)
        .unwrap_or_default()
}

/// Paints a selected or marked range across however many visual rows wrapping produced.
fn paint_wrapped_decoration(
    window: &mut Window,
    line: &WrappedLine,
    range: Range<usize>,
    origin: Point<Pixels>,
    width: Pixels,
    colour: gpui::Hsla,
    underline: bool,
) {
    if range.is_empty() {
        return;
    }
    let start = wrapped_position(line, range.start);
    let end = wrapped_position(line, range.end);
    let first = (start.y / AREA_LINE_HEIGHT).floor().max(0.0) as usize;
    let last = (end.y / AREA_LINE_HEIGHT).floor().max(0.0) as usize;
    for row in first..=last {
        let from = if row == first { start.x } else { px(0.0) };
        let to = if row == last { end.x } else { width };
        let (top, height) = if underline {
            (
                origin.y + AREA_LINE_HEIGHT * (row + 1) as f32 - px(3.0),
                px(1.5),
            )
        } else {
            (
                origin.y + AREA_LINE_HEIGHT * row as f32 + px(1.0),
                AREA_LINE_HEIGHT - px(2.0),
            )
        };
        paint::rect(
            window,
            Bounds {
                origin: point(origin.x + from, top),
                size: size((to - from).max(px(1.0)), height),
            },
            colour,
        );
    }
}

/// Paints prose with soft wrapping and keeps the caret inside the fixed-height viewport.
#[expect(
    clippy::too_many_arguments,
    reason = "one canvas, one bundle of what it draws"
)]
fn paint_wrapped_area(
    window: &mut Window,
    cx: &mut gpui::App,
    bounds: Bounds<Pixels>,
    text: &SharedString,
    selection: &Range<usize>,
    marked: Option<Range<usize>>,
    focused: bool,
    theme: &Theme,
) {
    let width = (bounds.size.width - FIELD_PADDING * 2.0).max(px(1.0));
    let mut run = window.text_style().to_run(text.len());
    run.color = theme.text;
    let Ok(shaped) =
        window
            .text_system()
            .shape_text(text.clone(), TEXT_SIZE, &[run], Some(width), None)
    else {
        return;
    };
    let ranges = lines(text);
    let watched = watched_offset(selection, marked.as_ref()).min(text.len());
    let watched_line = ranges
        .iter()
        .position(|range| watched >= range.start && watched <= range.end)
        .unwrap_or(ranges.len() - 1);
    let rows_before = shaped
        .iter()
        .take(watched_line)
        .map(|line| line.wrap_boundaries().len() + 1)
        .sum::<usize>();
    let watched_local = watched.saturating_sub(ranges[watched_line].start);
    let watched_position = wrapped_position(&shaped[watched_line], watched_local);
    let watched_row =
        rows_before + (watched_position.y / AREA_LINE_HEIGHT).floor().max(0.0) as usize;
    let visible_rows = ((bounds.size.height - AREA_PADDING_Y * 2.0) / AREA_LINE_HEIGHT)
        .floor()
        .max(1.0) as usize;
    let scroll_y = AREA_LINE_HEIGHT * first_visible_row(watched_row, visible_rows) as f32;
    let total_rows = shaped
        .iter()
        .map(|line| line.wrap_boundaries().len() + 1)
        .sum();
    WRAPPED_AREA.with(|area| area.set(Some((bounds, scroll_y, total_rows))));

    paint::clipped(window, bounds, |window| {
        let left = bounds.origin.x + FIELD_PADDING;
        let top = bounds.origin.y + AREA_PADDING_Y - scroll_y;
        let mut visual_row = 0usize;
        for (line, logical) in shaped.iter().zip(&ranges) {
            let origin = point(left, top + AREA_LINE_HEIGHT * visual_row as f32);
            let local_selection = Range {
                start: selection.start.clamp(logical.start, logical.end) - logical.start,
                end: selection.end.clamp(logical.start, logical.end) - logical.start,
            };
            if focused
                && !selection.is_empty()
                && selection.start <= logical.end
                && selection.end >= logical.start
            {
                paint_wrapped_decoration(
                    window,
                    line,
                    local_selection,
                    origin,
                    width,
                    Theme::translucent(theme.accent, 0.35),
                    false,
                );
            }
            let _ = line.paint(
                origin,
                AREA_LINE_HEIGHT,
                TextAlign::Left,
                Some(Bounds {
                    origin,
                    size: size(width, line.size(AREA_LINE_HEIGHT).height),
                }),
                window,
                cx,
            );
            if focused
                && let Some(marked) = &marked
                && marked.start <= logical.end
                && marked.end >= logical.start
            {
                let local = Range {
                    start: marked.start.clamp(logical.start, logical.end) - logical.start,
                    end: marked.end.clamp(logical.start, logical.end) - logical.start,
                };
                paint_wrapped_decoration(window, line, local, origin, width, theme.accent, true);
            }
            visual_row += line.wrap_boundaries().len() + 1;
        }

        let caret = point(
            left + watched_position.x,
            top + AREA_LINE_HEIGHT * watched_row as f32,
        );
        crate::ui::text_field::set_caret_bounds(Bounds {
            origin: caret,
            size: size(px(1.0), AREA_LINE_HEIGHT),
        });
        if focused && selection.is_empty() {
            paint::rect(
                window,
                Bounds {
                    origin: point(caret.x, caret.y + px(2.0)),
                    size: size(px(1.5), AREA_LINE_HEIGHT - px(4.0)),
                },
                theme.accent,
            );
        }
    });
}

#[cfg(test)]
pub(crate) fn wrapped_area_state() -> Option<(Pixels, usize)> {
    WRAPPED_AREA
        .with(Cell::get)
        .map(|(_, scroll_y, rows)| (scroll_y, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn typing_a_second_line_does_not_scroll_the_rendered_lyrics_editor(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.open_song_sheet();
            this.focus_section_lyrics(0);
        });
        crate::harness::paint(&app, cx);
        cx.simulate_input("さくら");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("さいた");
        crate::harness::paint(&app, cx);
        assert_eq!(
            AREA.with(Cell::get)
                .expect("the lyrics editor was painted")
                .2,
            px(0.0)
        );
        app.read_with(cx, |this, _| {
            assert_eq!(
                this.lyrics_edit.as_ref().unwrap().field.content(),
                "さくら\nさいた"
            );
        });
    }

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

    #[test]
    fn vertical_scroll_keeps_the_watched_row_visible() {
        assert_eq!(first_visible_row(0, 12), 0);
        assert_eq!(first_visible_row(11, 12), 0);
        assert_eq!(first_visible_row(12, 12), 1);
        assert_eq!(first_visible_row(39, 12), 28);
    }

    #[test]
    fn a_bordered_two_line_editor_keeps_the_first_line_visible_after_return() {
        let content_height = area_height("first\n", 2, 12) - px(2.0);
        let visible = ((content_height - AREA_PADDING_Y * 2.0) / AREA_LINE_HEIGHT).floor() as usize;
        assert_eq!(visible, 2);
        assert_eq!(first_visible_row(1, visible), 0);
        for count in 2..=12 {
            let height = area_height(&"line\n".repeat(count - 1), 2, 12) - px(2.0);
            let visible = ((height - AREA_PADDING_Y * 2.0) / AREA_LINE_HEIGHT).floor() as usize;
            assert_eq!(first_visible_row(count - 1, visible), 0);
        }
    }
}
