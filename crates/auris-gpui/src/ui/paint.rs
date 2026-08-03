//! Low-level painters shared by the canvas-backed views.
//!
//! The arrangement, the ruler and the piano roll draw hundreds to thousands of rectangles per
//! frame. Building that many `div`s would push the whole scene through layout every frame, so
//! these views use gpui's `canvas` element and paint quads directly. Everything here takes
//! plain data and a `Window`, which keeps the painting testable in isolation from view state
//! and reusable between panels.

use auris_session::prelude::*;

use gpui::{
    App, Bounds, ContentMask, Corners, Edges, Hsla, Pixels, Point, Window, fill, point, px, size,
};

use crate::theme::Theme;
use crate::ui::timeline::{PitchView, TimelineView};

/// Runs `f` with painting clipped to `bounds`.
pub fn clipped<R>(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    window.with_content_mask(Some(ContentMask { bounds }), f)
}

/// Fills a rectangle.
pub fn rect(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }
    window.paint_quad(fill(bounds, color));
}

/// Fills a rectangle with rounded corners.
///
/// The radius is clamped to half the shorter side so a small rectangle turns into a lozenge
/// rather than having its corners overlap and invert.
pub fn rounded_rect(window: &mut Window, bounds: Bounds<Pixels>, radius: Pixels, color: Hsla) {
    rect_with_corners(window, bounds, Corners::all(radius), color);
}

/// Fills a rectangle, rounding each corner independently.
pub fn rect_with_corners(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    radii: Corners<Pixels>,
    color: Hsla,
) {
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }
    let limit = bounds.size.width.min(bounds.size.height) / 2.0;
    let clamp = |value: Pixels| if value > limit { limit } else { value };
    window.paint_quad(fill(bounds, color).corner_radii(Corners {
        top_left: clamp(radii.top_left),
        top_right: clamp(radii.top_right),
        bottom_right: clamp(radii.bottom_right),
        bottom_left: clamp(radii.bottom_left),
    }));
}

/// Draws a rounded outline with no fill.
pub fn rounded_outline(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    radius: Pixels,
    width: Pixels,
    color: Hsla,
) {
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }
    let limit = bounds.size.width.min(bounds.size.height) / 2.0;
    window.paint_quad(gpui::quad(
        bounds,
        Corners::all(if radius > limit { limit } else { radius }),
        gpui::transparent_black(),
        Edges::all(width),
        color,
        gpui::BorderStyle::Solid,
    ));
}

/// Draws a one-pixel vertical line at `x` spanning the full height of `bounds`.
pub fn vline(window: &mut Window, bounds: Bounds<Pixels>, x: Pixels, width: Pixels, color: Hsla) {
    if x < bounds.origin.x - width || x > bounds.origin.x + bounds.size.width {
        return;
    }
    rect(
        window,
        Bounds {
            origin: point(x, bounds.origin.y),
            size: size(width, bounds.size.height),
        },
        color,
    );
}

/// Draws a one-pixel horizontal line at `y` spanning the full width of `bounds`.
pub fn hline(window: &mut Window, bounds: Bounds<Pixels>, y: Pixels, color: Hsla) {
    rect(
        window,
        Bounds {
            origin: point(bounds.origin.x, y),
            size: size(bounds.size.width, px(1.0)),
        },
        color,
    );
}

/// Paints a single line of text at `origin`, returning its width.
///
/// The caller gets the width back so labels can be laid out left to right without a second
/// shaping pass.
pub fn label(
    window: &mut Window,
    cx: &mut App,
    origin: Point<Pixels>,
    text: impl Into<gpui::SharedString>,
    font_size: Pixels,
    color: Hsla,
) -> Pixels {
    let text = text.into();
    if text.is_empty() {
        return px(0.0);
    }
    let mut run = window.text_style().to_run(text.len());
    run.color = color;
    let line = window
        .text_system()
        .shape_line(text, font_size, &[run], None);
    let width = line.width;
    // Ignore paint failures: a missing glyph must not take down the frame.
    let _ = line.paint(origin, font_size * 1.35, window, cx);
    width
}

/// Draws the rectangle being swept to select things.
///
/// A tinted fill and a solid outline: the fill says what is covered, and the outline keeps the
/// band visible where it lies over a clip that is already tinted the same accent colour.
pub fn selection_band(window: &mut Window, band: Bounds<Pixels>, theme: &Theme) {
    if band.size.width <= px(0.0) && band.size.height <= px(0.0) {
        return;
    }
    rect(window, band, Theme::translucent(theme.selection, 0.18));
    rounded_outline(window, band, px(0.0), px(1.0), theme.selection);
}

/// Draws the bar/beat/subdivision grid across `bounds`.
///
/// Lines get progressively brighter from subdivision to beat to bar so the eye can find the
/// downbeat without counting.
pub fn time_grid(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    view: &TimelineView,
    signature: TimeSignature,
    theme: &Theme,
) {
    let step = view.grid_step(signature, px(7.0));
    let ticks_per_bar = signature.ticks_per_bar().0.max(1);
    let ticks_per_beat = signature.ticks_per_beat().0.max(1);

    let (start, end) = view.visible_range(bounds.size.width);
    // Begin at the last gridline at or before the left edge so partially scrolled views still
    // show a line flush with the edge.
    let mut tick = start.snap_floor(step);
    while tick <= end {
        let x = bounds.origin.x + view.tick_to_x(tick);
        let (color, width) = if tick.0.rem_euclid(ticks_per_bar) == 0 {
            (theme.grid_bar, px(1.0))
        } else if tick.0.rem_euclid(ticks_per_beat) == 0 {
            (theme.grid_beat, px(1.0))
        } else {
            (theme.grid_subdivision, px(1.0))
        };
        vline(window, bounds, x, width, color);
        tick += step;
    }
}

/// Draws the bar-number ruler.
pub fn ruler(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    view: &TimelineView,
    signature: TimeSignature,
    theme: &Theme,
) {
    rect(window, bounds, theme.surface_raised);

    let ticks_per_bar = signature.ticks_per_bar().0.max(1);
    // Label every bar only while bars are wide enough to hold the text.
    let bar_width = ticks_per_bar as f32 * view.pixels_per_tick();
    let label_every = if bar_width >= 44.0 {
        1
    } else if bar_width >= 12.0 {
        4
    } else {
        16
    };

    let (start, end) = view.visible_range(bounds.size.width);
    let first_bar = (start.0.div_euclid(ticks_per_bar)).max(0);
    let last_bar = end.0.div_euclid(ticks_per_bar) + 1;

    for bar in first_bar..=last_bar {
        let tick = Ticks(bar * ticks_per_bar);
        let x = bounds.origin.x + view.tick_to_x(tick);
        if x < bounds.origin.x - px(40.0) || x > bounds.origin.x + bounds.size.width {
            continue;
        }
        let labelled = bar % label_every == 0;
        vline(
            window,
            bounds,
            x,
            px(1.0),
            if labelled {
                theme.grid_bar
            } else {
                theme.border_subtle
            },
        );
        if labelled {
            label(
                window,
                cx,
                point(x + px(4.0), bounds.origin.y + px(6.0)),
                format!("{}", bar + 1),
                px(10.0),
                theme.text_muted,
            );
        }
    }
}

/// Height of the strip the key changes get, along the top of the harmony lane.
pub const KEY_ROW_HEIGHT: Pixels = px(13.0);

/// Width of the grab zone on a chord's leading edge — the bar that moves it.
///
/// A chord *is* its start position, so its start edge is what there is to take hold of. The rest
/// of the block belongs to listening: pressing it sounds the chord and sweeping across the lane
/// plays the progression, and a drag anywhere would have taken that away.
pub const CHORD_HANDLE: Pixels = px(6.0);

/// The harmony lane's two rows: key changes above, chords below.
///
/// They shared one strip until a key change was found to land on the chord that begins a new
/// section far more often than not — which is exactly where the badge naming the key was printed
/// over the name of the chord underneath it. Both the painter and the hit test read this, so a
/// press lands on the row it looks like it lands on.
pub fn harmony_rows(bounds: Bounds<Pixels>) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let key_height = KEY_ROW_HEIGHT.min(bounds.size.height);
    (
        Bounds {
            origin: bounds.origin,
            size: size(bounds.size.width, key_height),
        },
        Bounds {
            origin: point(bounds.origin.x, bounds.origin.y + key_height),
            size: size(bounds.size.width, bounds.size.height - key_height),
        },
    )
}

/// What the harmony lane draws, copied out of the document for the paint closure.
#[derive(Clone, Debug, Default)]
pub struct HarmonyPaint {
    /// The chords sounding across the visible range, song-absolute and already clipped to it.
    pub events: Vec<HarmonicEvent>,
    /// Every key change in the song, in order.
    pub keys: Vec<KeyPoint>,
    /// Where a chord block may be taken hold of — see [`CHORD_HANDLE`].
    pub handles: Vec<Ticks>,
    /// The chord being dragged, drawn lit so it can be followed while it moves.
    pub held: Option<Ticks>,
}

/// Draws the chords, and the key changes, across `bounds`.
///
/// Each chord is a block from where it starts to where it ends, labelled with the numeral and —
/// when the block is wide enough to hold both — the chord that numeral means where it sits. A
/// cleared stretch has no event and so draws as bare background, which is the honest picture of
/// it.
pub fn harmony_lane(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    view: &TimelineView,
    harmony: &HarmonyPaint,
    theme: &Theme,
) {
    let (key_row, chord_row) = harmony_rows(bounds);
    // A shade below the ruler and above the lanes, so the three read as a descending stack rather
    // than as a hole cut in the middle of them. The key strip is sunk out of the way of the
    // chords, which are the part anybody is looking at.
    rect(window, chord_row, theme.surface);
    rect(window, key_row, theme.surface_sunken);

    for event in &harmony.events {
        let x = chord_row.origin.x + view.tick_to_x(event.start);
        let width = view.duration_to_width(event.length);
        if width <= px(1.0) {
            continue;
        }
        let lit = harmony.held == Some(event.start);
        let block = Bounds {
            origin: point(x + px(1.0), chord_row.origin.y + px(2.0)),
            size: size(width - px(2.0), chord_row.size.height - px(4.0)),
        };
        rounded_rect(
            window,
            block,
            px(3.0),
            Theme::translucent(theme.accent, if lit { 0.42 } else { 0.22 }),
        );
        // The handle is the visible half of the gesture: without something drawn at the edge,
        // being able to drag a chord there is a fact only the manual knows. Only where the
        // numeral is actually written — a chord split in two by a key change has one position,
        // and a handle on the far half would move music that starts off to the left of it.
        let movable = harmony.handles.contains(&event.start);
        if movable {
            rounded_rect(
                window,
                Bounds {
                    origin: block.origin,
                    size: size(CHORD_HANDLE.min(block.size.width), block.size.height),
                },
                px(3.0),
                if lit { theme.selection } else { theme.accent },
            );
        }

        // The numeral is what is stored and what transposes, so it leads; the sounding chord
        // follows only when there is room for it, since it is a readout rather than the truth.
        let text = if width >= px(96.0) {
            format!("{} · {}", event.numeral, event.name())
        } else {
            event.numeral.to_string()
        };
        clipped(window, block, |window| {
            label(
                window,
                cx,
                point(
                    block.origin.x + if movable { CHORD_HANDLE } else { px(0.0) } + px(4.0),
                    block.origin.y + (block.size.height - px(13.0)) / 2.0,
                ),
                text,
                px(10.0),
                theme.text,
            );
        });
    }

    let (start, end) = view.visible_range(bounds.size.width);
    // What key the view is scrolled into, pinned at the left edge. A song that modulates once in
    // bar 40 would otherwise show no key at all everywhere except bar 40, and reading a numeral
    // needs the key it is read against. Anything at or after the left edge is drawn where it
    // belongs by the loop below.
    if let Some(current) = harmony.keys.iter().rev().find(|change| change.tick < start) {
        let text = current.key.to_text();
        key_badge(
            window,
            cx,
            key_row,
            key_row.origin.x,
            &text,
            theme.text_faint,
        );
    }

    // A key change is a boundary rather than a span: it is drawn where it happens, named in the
    // strip above the chords, and marked by a line running down through them so the boundary is
    // visible where the music actually turns.
    for change in &harmony.keys {
        if change.tick < start || change.tick > end {
            continue;
        }
        let x = bounds.origin.x + view.tick_to_x(change.tick);
        clipped(window, bounds, |window| {
            vline(window, chord_row, x, px(1.5), theme.selection);
        });
        key_badge(
            window,
            cx,
            key_row,
            x,
            &change.key.to_text(),
            theme.selection,
        );
    }
}

/// One key name in the strip above the chords, starting at `x`.
fn key_badge(
    window: &mut Window,
    cx: &mut App,
    row: Bounds<Pixels>,
    x: Pixels,
    text: &str,
    color: Hsla,
) {
    // The badge has to be drawn before the text sits on it, and `label` only reports its width
    // after drawing, so the width is estimated. Over- or under-shooting costs a little padding,
    // which is why the estimate is generous rather than exact.
    let badge = Bounds {
        origin: point(x, row.origin.y),
        size: size(
            px(text.chars().count() as f32 * 6.0 + 10.0),
            row.size.height,
        ),
    };
    clipped(window, row, |window| {
        rounded_rect(window, badge, px(2.0), Theme::translucent(color, 0.14));
        label(
            window,
            cx,
            point(x + px(5.0), row.origin.y + px(1.0)),
            text.to_string(),
            px(9.0),
            color,
        );
    });
}

/// Tints the loop region across `bounds`.
pub fn loop_region(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    view: &TimelineView,
    region: (Ticks, Ticks),
    theme: &Theme,
) {
    let (start, end) = region;
    if end <= start {
        return;
    }
    // Clamp both edges before sizing. Clamping only the left one and then taking the unclamped
    // width paints past the right edge, and paints at all for a region scrolled off-screen.
    let left = (bounds.origin.x + view.tick_to_x(start)).max(bounds.origin.x);
    let right = (bounds.origin.x + view.tick_to_x(end)).min(bounds.origin.x + bounds.size.width);
    if right <= left {
        return;
    }
    rect(
        window,
        Bounds {
            origin: point(left, bounds.origin.y),
            size: size(right - left, bounds.size.height),
        },
        Theme::translucent(theme.loop_region, 0.18),
    );
}

/// Draws the playhead line, with a small triangle-ish head at the top.
pub fn playhead(window: &mut Window, bounds: Bounds<Pixels>, x: Pixels, theme: &Theme) {
    if x < bounds.origin.x || x > bounds.origin.x + bounds.size.width {
        return;
    }
    vline(window, bounds, x, px(1.5), theme.playhead);
    // A downward flag rather than a block, so the point marks the exact position the line
    // runs through — the same shape every DAW puts at the top of its playhead.
    let top = bounds.origin.y;
    let mut builder = gpui::PathBuilder::fill();
    builder.move_to(point(x - px(5.0), top));
    builder.line_to(point(x + px(5.0), top));
    builder.line_to(point(x, top + px(7.0)));
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, theme.playhead);
    }
}

/// Draws a miniature note preview inside a clip rectangle.
///
/// The preview is scaled to the pitch range actually used by the clip, so a two-note bass part
/// and a dense chord voicing both fill the available height.
pub fn clip_notes(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    notes: &[Note],
    clip_length: Ticks,
    color: Hsla,
) {
    if notes.is_empty() || clip_length.0 <= 0 || bounds.size.height <= px(6.0) {
        return;
    }

    let (lowest, highest) = notes.iter().fold((u8::MAX, u8::MIN), |(lo, hi), note| {
        (lo.min(note.pitch), hi.max(note.pitch))
    });
    // Give a single-pitch clip a nominal range so its notes land mid-height instead of at 0/0.
    let span = (highest.saturating_sub(lowest)).max(1) as f32;
    let usable = f32::from(bounds.size.height) - 4.0;
    let note_height = (usable / (span + 1.0)).clamp(1.0, 6.0);
    let x_scale = f32::from(bounds.size.width) / clip_length.0 as f32;

    for note in notes {
        if note.start >= clip_length {
            continue;
        }
        let visible_length = (note.length.0).min(clip_length.0 - note.start.0).max(1);
        let x = f32::from(bounds.origin.x) + note.start.0 as f32 * x_scale;
        let width = (visible_length as f32 * x_scale).max(1.5);
        let from_top = (highest - note.pitch) as f32 / span;
        let y = f32::from(bounds.origin.y) + 2.0 + from_top * (usable - note_height);
        rect(
            window,
            Bounds {
                origin: point(px(x), px(y)),
                size: size(px(width), px(note_height)),
            },
            color,
        );
    }
}

/// Draws a min/max waveform from precomputed peaks.
///
/// `min` and `max` are one entry per horizontal bucket; `first_bucket` lets a clip that is
/// scrolled or trimmed start part-way into the peak data without re-computing it.
pub fn waveform(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    min: &[f32],
    max: &[f32],
    first_bucket: usize,
    buckets_per_pixel: f32,
    color: Hsla,
) {
    if min.is_empty() || bounds.size.height <= px(2.0) {
        return;
    }
    let mid = f32::from(bounds.origin.y) + f32::from(bounds.size.height) / 2.0;
    let half = f32::from(bounds.size.height) / 2.0 - 1.0;
    let columns = f32::from(bounds.size.width).max(0.0) as usize;

    for column in 0..columns {
        // Each screen column may cover several peak buckets once a clip is zoomed out; take the
        // extremes across them so transients never disappear between frames.
        let from = first_bucket + (column as f32 * buckets_per_pixel) as usize;
        let to = (first_bucket + ((column + 1) as f32 * buckets_per_pixel) as usize).max(from + 1);
        if from >= min.len() {
            break;
        }
        let to = to.min(min.len());
        let low = min[from..to].iter().copied().fold(f32::MAX, f32::min);
        let high = max[from..to].iter().copied().fold(f32::MIN, f32::max);
        if !low.is_finite() || !high.is_finite() {
            continue;
        }
        let top = mid - high.clamp(-1.0, 1.0) * half;
        let bottom = mid - low.clamp(-1.0, 1.0) * half;
        rect(
            window,
            Bounds {
                origin: point(bounds.origin.x + px(column as f32), px(top)),
                size: size(px(1.0), px((bottom - top).max(1.0))),
            },
            color,
        );
    }
}

/// Draws the piano-roll row backgrounds and horizontal separators.
pub fn pitch_rows(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    pitch_view: &PitchView,
    theme: &Theme,
) {
    let rows = (f32::from(bounds.size.height) / pitch_view.row_height).ceil() as i32 + 1;
    for row in 0..rows {
        let pitch = pitch_view.top_pitch as i32 - row;
        if !(0..=127).contains(&pitch) {
            continue;
        }
        let y = bounds.origin.y + px(row as f32 * pitch_view.row_height);
        if super::timeline::is_black_key(pitch as u8) {
            rect(
                window,
                Bounds {
                    origin: point(bounds.origin.x, y),
                    size: size(bounds.size.width, px(pitch_view.row_height)),
                },
                theme.key_row_black,
            );
        }
        // A brighter separator under every B/C boundary marks the octave.
        if pitch % 12 == 0 {
            hline(
                window,
                bounds,
                y + px(pitch_view.row_height),
                theme.grid_bar,
            );
        }
    }
}

/// Draws the piano keyboard strip down the left of the roll.
pub fn keyboard(
    window: &mut Window,
    cx: &mut App,
    bounds: Bounds<Pixels>,
    pitch_view: &PitchView,
    theme: &Theme,
) {
    rect(window, bounds, theme.surface);
    let rows = (f32::from(bounds.size.height) / pitch_view.row_height).ceil() as i32 + 1;
    for row in 0..rows {
        let pitch = pitch_view.top_pitch as i32 - row;
        if !(0..=127).contains(&pitch) {
            continue;
        }
        let pitch = pitch as u8;
        let y = bounds.origin.y + px(row as f32 * pitch_view.row_height);
        let black = super::timeline::is_black_key(pitch);
        // Only the inner end of a key is rounded: the outer end butts against the panel edge
        // exactly as the keys of a real keyboard butt against the cheek block.
        let radius = px((pitch_view.row_height * 0.25).min(3.0));
        rect_with_corners(
            window,
            Bounds {
                origin: point(bounds.origin.x, y),
                // Black keys are drawn short, as on a real keyboard.
                size: size(
                    if black {
                        bounds.size.width * 0.62
                    } else {
                        bounds.size.width
                    },
                    px(pitch_view.row_height - 1.0),
                ),
            },
            Corners {
                top_left: px(0.0),
                bottom_left: px(0.0),
                top_right: radius,
                bottom_right: radius,
            },
            if black {
                theme.key_black
            } else {
                theme.key_white
            },
        );
        // Only C gets a name, otherwise the strip is unreadable at small row heights.
        if pitch.is_multiple_of(12) && pitch_view.row_height >= 9.0 {
            label(
                window,
                cx,
                point(
                    bounds.origin.x + bounds.size.width - px(24.0),
                    y + px((pitch_view.row_height - 9.0) / 2.0),
                ),
                super::timeline::pitch_name(pitch),
                px(9.0),
                // The colour of the black keys, because it is on a white one. It used to be the
                // recessed surface, which happens to be dark in a dark scheme and is near-white
                // in a light one — where the label would vanish into the key it names.
                theme.key_black,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_harmony_lane_gives_the_keys_a_strip_of_their_own() {
        // Two rows that tile the lane exactly: a gap between them would be a band of pixels no
        // hit test claims, and an overlap would make a press on a chord land on a key change.
        let bounds = Bounds {
            origin: point(px(10.0), px(40.0)),
            size: size(px(800.0), crate::theme::Metrics::HARMONY_LANE_HEIGHT),
        };
        let (keys, chords) = harmony_rows(bounds);
        assert_eq!(keys.origin, bounds.origin);
        assert_eq!(keys.size.height, KEY_ROW_HEIGHT);
        assert_eq!(chords.origin.y, keys.origin.y + keys.size.height);
        assert_eq!(chords.size.height + keys.size.height, bounds.size.height);
        assert!(
            chords.size.height > keys.size.height,
            "the chords are what anybody is looking at"
        );
    }

    #[test]
    fn a_lane_too_short_for_two_rows_still_gives_both_a_real_rectangle() {
        // Nothing lays the lane out this small today, but a negative height would paint the key
        // strip over the whole window rather than merely looking wrong.
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(800.0), px(6.0)),
        };
        let (keys, chords) = harmony_rows(bounds);
        assert_eq!(keys.size.height, px(6.0));
        assert_eq!(chords.size.height, px(0.0));
    }
}
