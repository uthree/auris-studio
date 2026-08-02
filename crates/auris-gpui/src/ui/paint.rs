//! Low-level painters shared by the canvas-backed views.
//!
//! The arrangement, the ruler and the piano roll draw hundreds to thousands of rectangles per
//! frame. Building that many `div`s would push the whole scene through layout every frame, so
//! these views use gpui's `canvas` element and paint quads directly. Everything here takes
//! plain data and a `Window`, which keeps the painting testable in isolation from view state
//! and reusable between panels.

use auris_session::prelude::*;

use gpui::{App, Bounds, ContentMask, Hsla, Pixels, Point, Window, fill, point, px, size};

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
pub fn rounded_rect(window: &mut Window, bounds: Bounds<Pixels>, radius: Pixels, color: Hsla) {
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }
    window.paint_quad(fill(bounds, color).corner_radii(radius));
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
    rect(
        window,
        Bounds {
            origin: point(x - px(4.0), bounds.origin.y),
            size: size(px(9.0), px(5.0)),
        },
        theme.playhead,
    );
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
        rect(
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
                theme.surface_sunken,
            );
        }
    }
}
