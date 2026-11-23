//! Labelled axis ticks for the pictures shown to a model.
//!
//! A picture without numbers on its axes leaves a model estimating "about a third of the way up"
//! and converting that into hertz or bars itself, which is where readings go wrong. Every image
//! a tool returns therefore carries a few major ticks with their values written beside them, in a
//! margin outside the data so no measurement is covered.
//!
//! The labels are drawn from a small built-in bitmap font rather than an installed one: the
//! picture must be the same on every machine (and in CI, which has no fonts), and the labels
//! only ever need digits, note names, a few unit letters, and `/ . - # :`. Each glyph is 5x7 cells
//! drawn at [`SCALE`], large enough to survive the downscaling a model's image encoder applies.
//! Labels are kept sparse — a candidate is dropped when it would sit closer than a minimum gap to
//! one already placed — because a crowded axis is misread more often than a sparse one.
use image::{Rgb, RgbImage};

/// Pixels per font cell.
pub(crate) const SCALE: u32 = 2;
/// Height of a line of label text, in pixels.
pub(crate) const TEXT_HEIGHT: u32 = 7 * SCALE;
/// Horizontal distance from one character to the next, in pixels.
const ADVANCE: u32 = 6 * SCALE;
/// Background of every margin.
pub(crate) const MARGIN: Rgb<u8> = Rgb([12, 16, 24]);
/// Label text.
pub(crate) const INK: Rgb<u8> = Rgb([228, 231, 238]);
/// Tick marks and panel frames.
pub(crate) const TICK: Rgb<u8> = Rgb([128, 134, 148]);
/// Length of a tick mark, in pixels.
pub(crate) const TICK_LENGTH: u32 = 5;
/// The fewest pixels between the centres of two labels on one axis.
pub(crate) const MIN_LABEL_GAP: f64 = 18.0;

/// The 5x7 cells of `c`, top row first, or `None` for a character the font does not have.
fn glyph(c: char) -> Option<[&'static str; 7]> {
    Some(match c {
        '0' => [
            ".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###.",
        ],
        '1' => [
            "..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###.",
        ],
        '2' => [
            ".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####",
        ],
        '3' => [
            "#####", "...#.", "..#..", "...#.", "....#", "#...#", ".###.",
        ],
        '4' => [
            "...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#.",
        ],
        '5' => [
            "#####", "#....", "####.", "....#", "....#", "#...#", ".###.",
        ],
        '6' => [
            "..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###.",
        ],
        '7' => [
            "#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#...",
        ],
        '8' => [
            ".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###.",
        ],
        '9' => [
            ".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##..",
        ],
        '/' => [
            "....#", "....#", "...#.", "..#..", ".#...", "#....", "#....",
        ],
        '.' => [
            ".....", ".....", ".....", ".....", ".....", ".##..", ".##..",
        ],
        ':' => [
            ".....", ".##..", ".##..", ".....", ".##..", ".##..", ".....",
        ],
        '-' => [
            ".....", ".....", ".....", ".###.", ".....", ".....", ".....",
        ],
        '#' => [
            ".#.#.", ".#.#.", "#####", ".#.#.", "#####", ".#.#.", ".#.#.",
        ],
        ' ' => [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
        'A' => [
            ".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
        'B' => [
            "####.", "#...#", "#...#", "####.", "#...#", "#...#", "####.",
        ],
        'C' => [
            ".###.", "#...#", "#....", "#....", "#....", "#...#", ".###.",
        ],
        'D' => [
            "###..", "#..#.", "#...#", "#...#", "#...#", "#..#.", "###..",
        ],
        'E' => [
            "#####", "#....", "#....", "####.", "#....", "#....", "#####",
        ],
        'F' => [
            "#####", "#....", "#....", "####.", "#....", "#....", "#....",
        ],
        'G' => [
            ".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".####",
        ],
        'H' => [
            "#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ],
        'a' => [
            ".....", ".....", ".###.", "....#", ".####", "#...#", ".####",
        ],
        'b' => [
            "#....", "#....", "#.##.", "##..#", "#...#", "#...#", "####.",
        ],
        'c' => [
            ".....", ".....", ".###.", "#....", "#....", "#...#", ".###.",
        ],
        'e' => [
            ".....", ".....", ".###.", "#...#", "#####", "#....", ".###.",
        ],
        'k' => [
            "#....", "#....", "#..#.", "#.#..", "##...", "#.#..", "#..#.",
        ],
        'l' => [
            ".##..", "..#..", "..#..", "..#..", "..#..", "..#..", ".###.",
        ],
        'p' => [
            ".....", ".....", "####.", "#...#", "####.", "#....", "#....",
        ],
        'r' => [
            ".....", ".....", "#.##.", "##..#", "#....", "#....", "#....",
        ],
        's' => [
            ".....", ".....", ".####", "#....", ".###.", "....#", "####.",
        ],
        't' => [
            ".#...", ".#...", "###..", ".#...", ".#...", ".#..#", "..##.",
        ],
        'x' => [
            ".....", ".....", "#...#", ".#.#.", "..#..", ".#.#.", "#...#",
        ],
        'y' => [
            ".....", ".....", "#...#", "#...#", ".####", "....#", ".###.",
        ],
        'z' => [
            ".....", ".....", "#####", "...#.", "..#..", ".#...", "#####",
        ],
        _ => return None,
    })
}

/// Width of `text` in pixels, from the left of its first cell to the right of its last.
pub(crate) fn text_width(text: &str) -> u32 {
    let count = text.chars().count() as u32;
    (count * ADVANCE).saturating_sub(SCALE)
}

/// Paint `text` with its top-left corner at (`x`, `y`), clipped to the image.
pub(crate) fn draw_text(image: &mut RgbImage, x: i64, y: i64, text: &str, ink: Rgb<u8>) {
    let (width, height) = image.dimensions();
    for (index, c) in text.chars().enumerate() {
        debug_assert!(glyph(c).is_some(), "no glyph for {c:?}");
        let Some(rows) = glyph(c) else { continue };
        let left = x + index as i64 * i64::from(ADVANCE);
        for (row, cells) in rows.iter().enumerate() {
            for (column, cell) in cells.bytes().enumerate() {
                if cell != b'#' {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let px = left + (column as u32 * SCALE + dx) as i64;
                        let py = y + (row as u32 * SCALE + dy) as i64;
                        if (0..i64::from(width)).contains(&px)
                            && (0..i64::from(height)).contains(&py)
                        {
                            image.put_pixel(px as u32, py as u32, ink);
                        }
                    }
                }
            }
        }
    }
}

/// A y-axis label: right-aligned to end at `right`, centred on `y` but kept inside `top..bottom`.
pub(crate) fn label_left(
    image: &mut RgbImage,
    right: u32,
    y: f64,
    top: u32,
    bottom: u32,
    text: &str,
) {
    let x = i64::from(right) - i64::from(text_width(text));
    let highest = i64::from(top);
    let lowest = (i64::from(bottom) - i64::from(TEXT_HEIGHT)).max(highest);
    let y = (y.round() as i64 - i64::from(TEXT_HEIGHT / 2)).clamp(highest, lowest);
    draw_text(image, x, y, text, INK);
}

/// An x-axis label: centred on `x` with its top at `top`, kept right of `leftmost` and inside
/// the image, so the first label of an axis starts at the axis rather than in the margin.
pub(crate) fn label_below(image: &mut RgbImage, x: f64, top: u32, leftmost: u32, text: &str) {
    let width = i64::from(text_width(text));
    let right = (i64::from(image.width()) - width).max(0);
    let left = (x.round() as i64 - width / 2).clamp(i64::from(leftmost).min(right), right);
    draw_text(image, left, i64::from(top), text, INK);
}

/// A horizontal tick ending one pixel left of `right`.
pub(crate) fn tick_left(image: &mut RgbImage, right: u32, y: f64) {
    let y = y.round().clamp(0.0, f64::from(image.height() - 1)) as u32;
    for x in right.saturating_sub(TICK_LENGTH + 1)..right.saturating_sub(1) {
        image.put_pixel(x, y, TICK);
    }
}

/// A vertical tick starting one pixel below `top`.
pub(crate) fn tick_below(image: &mut RgbImage, x: f64, top: u32) {
    let x = x.round().clamp(0.0, f64::from(image.width() - 1)) as u32;
    for y in top + 1..(top + 1 + TICK_LENGTH).min(image.height()) {
        image.put_pixel(x, y, TICK);
    }
}

/// A one-pixel frame just outside the rectangle `left..right` by `top..bottom`.
pub(crate) fn frame(image: &mut RgbImage, left: u32, top: u32, right: u32, bottom: u32) {
    let (width, height) = image.dimensions();
    let x0 = left.saturating_sub(1);
    let y0 = top.saturating_sub(1);
    for x in x0..=right.min(width - 1) {
        for y in [y0, bottom.min(height - 1)] {
            image.put_pixel(x, y, TICK);
        }
    }
    for y in y0..=bottom.min(height - 1) {
        for x in [x0, right.min(width - 1)] {
            image.put_pixel(x, y, TICK);
        }
    }
}

/// The candidates that fit, most important tier first: a candidate is kept when it is at least
/// `gap` pixels from every one kept before it. Returned in position order.
pub(crate) fn sparse<T>(tiers: Vec<Vec<(f64, T)>>, gap: f64) -> Vec<(f64, T)> {
    let mut kept: Vec<(f64, T)> = Vec::new();
    for tier in tiers {
        for (at, value) in tier {
            if at.is_finite() && kept.iter().all(|(other, _)| (other - at).abs() >= gap) {
                kept.push((at, value));
            }
        }
    }
    kept.sort_by(|a, b| a.0.total_cmp(&b.0));
    kept
}

/// A frequency as a label: whole hertz below 1 kHz, kilohertz with a `k` above.
pub(crate) fn hz_label(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{}k", (hz / 10.0).round() / 100.0)
    } else if hz >= 100.0 {
        format!("{}", hz.round())
    } else {
        format!("{}", (hz * 10.0).round() / 10.0)
    }
}

/// A MIDI pitch in scientific notation, middle C (60) being `C4`.
pub(crate) fn pitch_label(pitch: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(pitch % 12)],
        i32::from(pitch / 12) - 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_the_pictures_write_has_glyphs() {
        for text in [
            "0123456789",
            "1/16",
            "31.3",
            "1.26k",
            "C#-1",
            "G9",
            "bar",
            "x: cycles/beat",
            "12: bars 9..17",
            "ABCDEFGH",
        ] {
            assert!(text.chars().all(|c| glyph(c).is_some()), "{text}");
        }
        for rows in "0123456789/.:-# ABCDEFGHabceklprstxyz"
            .chars()
            .filter_map(glyph)
        {
            assert!(rows.iter().all(|row| row.len() == 5));
        }
    }

    #[test]
    fn text_is_drawn_at_the_scale_and_clipped_at_the_edge() {
        let mut image = RgbImage::from_pixel(40, 20, MARGIN);
        draw_text(&mut image, 0, 0, "1", INK);
        // The 1's top cell is the third column, two pixels per cell.
        assert_eq!(image.get_pixel(4, 0), &INK);
        assert_eq!(image.get_pixel(5, 1), &INK);
        assert_eq!(image.get_pixel(0, 0), &MARGIN);
        assert_eq!(text_width("10"), 22);
        draw_text(&mut image, 35, 15, "8", INK);
        draw_text(&mut image, -100, -100, "8", INK);
    }

    #[test]
    fn labels_read_like_the_numbers_in_the_text() {
        assert_eq!(hz_label(31.25), "31.3");
        assert_eq!(hz_label(125.0), "125");
        assert_eq!(hz_label(1000.0), "1k");
        assert_eq!(hz_label(1260.0), "1.26k");
        assert_eq!(hz_label(16000.0), "16k");
        assert_eq!(pitch_label(60), "C4");
        assert_eq!(pitch_label(0), "C-1");
        assert_eq!(pitch_label(127), "G9");
        assert_eq!(pitch_label(61), "C#4");
    }

    #[test]
    fn sparse_keeps_the_important_tier_and_drops_crowded_candidates() {
        let kept = sparse(
            vec![
                vec![(0.0, "a"), (100.0, "b")],
                vec![(10.0, "c"), (50.0, "d")],
            ],
            18.0,
        );
        assert_eq!(kept, vec![(0.0, "a"), (50.0, "d"), (100.0, "b")]);
    }
}
