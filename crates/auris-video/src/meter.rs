//! The tempo and the beat counter, in the top right corner, across from the chord caption.
//!
//! The counter is a regular polygon with a corner for each beat of the bar, the first at the
//! top and the rest clockwise. The corner of the beat sounding now is lit and swells as the beat
//! is struck, the corners already played stay lit, and the outline is traced round from the
//! top as the bar goes by — so a glance tells where in the bar the music is, and the shape
//! itself says the metre: a triangle is a waltz, a square a common-time bar, a hexagon a bar of
//! 6/8. The downbeat flashes the whole outline. The beat's number sits in the middle.
//!
//! A bar of one or two beats has no polygon to be, so its corners sit on a circle instead.
//!
//! The tempo is written beside the counter, or alone in the corner when the counter is off. The
//! figures are outlined from the same installed font as the chord names; a machine with no font
//! draws the counter's shape without its number, and no tempo.

use std::collections::HashMap;
use std::f64::consts::TAU;

use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path as SkPath, PathBuilder, Pixmap, Stroke, Transform,
};

use crate::chords::{Label, capital_height, outline, system_font, unit_byte};
use crate::{NoteRollStyle, NoteScore};

/// Space from the frame's top and right edges, as a fraction of its shorter side — the chord
/// caption's margin, so the two corners line up.
const MARGIN: f64 = 0.045;
/// The counter's radius, as a fraction of the frame's shorter side.
const RADIUS: f64 = 0.05;
/// Height of the tempo's figures, as a fraction of the frame's shorter side.
const TEMPO_SIZE: f64 = 0.042;
/// The unit after the figures, as a fraction of their height.
const UNIT_SCALE: f64 = 0.5;
/// Space between the tempo and the counter, as a fraction of the frame's shorter side.
const GAP: f64 = 0.03;
/// How long the swell of a struck beat takes to settle, in seconds.
const PULSE_SECONDS: f64 = 0.16;
/// Opacity of the part of the outline the bar has not reached yet.
const TRACK_OPACITY: f64 = 0.28;
/// Opacity of the corners still to come.
const AHEAD_OPACITY: f64 = 0.35;
/// The unit written after the tempo.
const UNIT: &str = "BPM";

/// Every character the overlay writes.
const CHARACTERS: &str = "0123456789.";

/// The overlay for one score, with every figure it writes already outlined.
pub(crate) struct MeterOverlay {
    tempo: bool,
    counter: bool,
    /// Each digit and the decimal point, outlined in font units, when there is a font.
    glyphs: HashMap<char, Label>,
    unit: Option<Label>,
    /// Height of a capital in font units.
    cap: f64,
}

impl MeterOverlay {
    /// The overlay for `score` in `style`, or `None` when the style shows neither part or the
    /// score has nothing for them to show.
    pub(crate) fn new(score: &NoteScore, style: &NoteRollStyle) -> Option<Self> {
        let counter = style.beat_counter && !score.beats.is_empty();
        let mut overlay = Self {
            tempo: false,
            counter,
            glyphs: HashMap::new(),
            unit: None,
            cap: 1.0,
        };
        let face =
            system_font().and_then(|font| ttf_parser::Face::parse(&font.bytes, font.index).ok());
        if let Some(face) = face {
            overlay.tempo = style.tempo && !score.tempos.is_empty();
            overlay.cap = capital_height(&face);
            for character in CHARACTERS.chars() {
                overlay
                    .glyphs
                    .insert(character, outline(&face, &character.to_string()));
            }
            overlay.unit = Some(outline(&face, UNIT));
        }
        (overlay.tempo || overlay.counter).then_some(overlay)
    }

    /// Draws the overlay for the moment `seconds` into `frame`.
    pub(crate) fn draw(
        &self,
        score: &NoteScore,
        style: &NoteRollStyle,
        seconds: f64,
        frame: &mut Pixmap,
    ) {
        let width = f64::from(style.size.width);
        let height = f64::from(style.size.height);
        let short = width.min(height);
        let margin = short * MARGIN;
        let radius = short * RADIUS;
        let centre = (width - margin - radius, margin + radius);

        let mut right = width - margin;
        if self.counter {
            if let Some(index) = score.beat_at(seconds) {
                self.draw_counter(score, index, seconds, centre, radius, style, frame);
            }
            right = centre.0 - radius - short * GAP;
        }
        if self.tempo
            && let Some(bpm) = score.tempo_at(seconds)
        {
            let cap = short * TEMPO_SIZE;
            // Centred on the counter, so the two read as one line.
            let baseline = centre.1 + cap / 2.0;
            let unit_cap = cap * UNIT_SCALE;
            let unit_width =
                self.unit.as_ref().map_or(0.0, |unit| unit.advance) * unit_cap / self.cap;
            let figures = tempo_text(bpm);
            let figures_width = self.width(&figures, cap);
            let x = right - unit_width - cap * 0.3 - figures_width;
            self.write(&figures, x, baseline, cap, 1.0, style, frame);
            if let Some(unit) = &self.unit {
                self.glyph(
                    unit,
                    right - unit_width,
                    baseline,
                    unit_cap,
                    0.7,
                    style,
                    frame,
                );
            }
        }
    }

    /// The polygon, its corners, and the number of the beat at `index`.
    #[allow(clippy::too_many_arguments)]
    fn draw_counter(
        &self,
        score: &NoteScore,
        index: usize,
        seconds: f64,
        centre: (f64, f64),
        radius: f64,
        style: &NoteRollStyle,
        frame: &mut Pixmap,
    ) {
        let beat = score.beats[index];
        let corners = beat.beats.max(1);
        let length = (beat.end - beat.start).max(1e-6);
        let into = ((seconds - beat.start) / length).clamp(0.0, 1.0);
        let pulse = (-(seconds - beat.start).max(0.0) / PULSE_SECONDS).exp();
        let downbeat = if beat.beat == 1 { pulse } else { 0.0 };
        let shape = Shape {
            corners,
            centre,
            radius,
        };
        let line = (radius * 0.05).max(1.5);

        // The whole outline faintly, flashing on the downbeat; then the part the bar has
        // reached, traced from the top.
        let whole = shape.trace(f64::from(corners));
        let traced = shape.trace(f64::from(beat.beat - 1) + into);
        for (path, width, opacity) in [
            (
                &whole,
                line,
                TRACK_OPACITY + (1.0 - TRACK_OPACITY) * 0.6 * downbeat,
            ),
            (&traced, line * 1.4, 0.9),
        ] {
            if let Some(path) = path {
                stroke(frame, path, style, width, opacity);
            }
        }

        // The corners: those played lit, the one sounding now swollen as it is struck, those
        // to come dim. They shrink as there are more of them, so neighbours never touch.
        let edge = 2.0 * radius * (std::f64::consts::PI / f64::from(corners.max(3))).sin();
        let dot = (radius * 0.12).min(edge * 0.22).max(1.5);
        for corner in 0..corners {
            let (x, y) = shape.corner(corner);
            let (size, opacity) = match (corner + 1).cmp(&beat.beat) {
                std::cmp::Ordering::Less => (dot, 0.85),
                std::cmp::Ordering::Equal => (dot * (1.25 + 0.7 * pulse), 1.0),
                std::cmp::Ordering::Greater => (dot * 0.8, AHEAD_OPACITY),
            };
            if let Some(circle) = PathBuilder::from_circle(x as f32, y as f32, size as f32) {
                shadow(frame, &circle, size * 0.25);
                fill(frame, &circle, style, opacity, Transform::identity());
            }
        }

        // The beat's number in the middle, sized to fit inside the smallest polygon.
        let number = beat.beat.to_string();
        let cap = radius * if corners == 3 { 0.42 } else { 0.55 };
        // A triangle's middle sits low of its circle's centre; the number follows it.
        let middle = if corners == 3 {
            centre.1 + radius * 0.12
        } else {
            centre.1
        };
        let x = centre.0 - self.width(&number, cap) / 2.0;
        self.write(&number, x, middle + cap / 2.0, cap, 0.95, style, frame);
    }

    /// How wide `text` is written with capitals `cap` pixels tall.
    fn width(&self, text: &str, cap: f64) -> f64 {
        text.chars()
            .filter_map(|character| self.glyphs.get(&character))
            .map(|glyph| glyph.advance * cap / self.cap)
            .sum()
    }

    /// Writes `text`, made only of [`CHARACTERS`], from `x` along `baseline`.
    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        text: &str,
        mut x: f64,
        baseline: f64,
        cap: f64,
        opacity: f64,
        style: &NoteRollStyle,
        frame: &mut Pixmap,
    ) {
        for character in text.chars() {
            if let Some(glyph) = self.glyphs.get(&character) {
                x += self.glyph(glyph, x, baseline, cap, opacity, style, frame);
            }
        }
    }

    /// Fills one outlined label over a soft shadow. Returns how wide it was.
    #[allow(clippy::too_many_arguments)]
    fn glyph(
        &self,
        label: &Label,
        x: f64,
        baseline: f64,
        cap: f64,
        opacity: f64,
        style: &NoteRollStyle,
        frame: &mut Pixmap,
    ) -> f64 {
        let scale = cap / self.cap;
        if let Some(path) = &label.path {
            let place = |dy: f64| {
                Transform::from_row(
                    scale as f32,
                    0.0,
                    0.0,
                    -scale as f32,
                    x as f32,
                    (baseline + dy) as f32,
                )
            };
            let mut paint = Paint {
                anti_alias: true,
                ..Paint::default()
            };
            paint.set_color_rgba8(0, 0, 0, unit_byte(0.45 * opacity));
            frame.fill_path(
                path,
                &paint,
                FillRule::Winding,
                place((cap * 0.05).max(1.0)),
                None,
            );
            fill(frame, path, style, opacity, place(0.0));
        }
        label.advance * scale
    }
}

/// A tempo as it is written: whole when it is whole, to a tenth when it is not.
fn tempo_text(bpm: f64) -> String {
    let tenths = (bpm * 10.0).round();
    if tenths % 10.0 == 0.0 {
        format!("{:.0}", tenths / 10.0)
    } else {
        format!("{:.1}", tenths / 10.0)
    }
}

/// The counter's outline: `corners` points round a circle, the first at the top.
struct Shape {
    corners: u32,
    centre: (f64, f64),
    radius: f64,
}

impl Shape {
    /// Where corner `index` sits.
    fn corner(&self, index: u32) -> (f64, f64) {
        self.at_angle(TAU * f64::from(index) / f64::from(self.corners))
    }

    fn at_angle(&self, angle: f64) -> (f64, f64) {
        (
            self.centre.0 + self.radius * angle.sin(),
            self.centre.1 - self.radius * angle.cos(),
        )
    }

    /// The outline from the top as far as `along` beats round it — along the edges of the
    /// polygon, or round the circle for a bar of one or two beats.
    fn trace(&self, along: f64) -> Option<SkPath> {
        let along = along.clamp(0.0, f64::from(self.corners));
        if along <= 0.0 {
            return None;
        }
        let mut path = PathBuilder::new();
        let (x, y) = self.corner(0);
        path.move_to(x as f32, y as f32);
        if self.corners < 3 {
            let steps = (along / f64::from(self.corners) * 72.0).ceil().max(1.0) as u32;
            for step in 1..=steps {
                let angle =
                    TAU * along / f64::from(self.corners) * f64::from(step) / f64::from(steps);
                let (x, y) = self.at_angle(angle);
                path.line_to(x as f32, y as f32);
            }
        } else {
            let whole = along.floor() as u32;
            for corner in 1..=whole {
                let (x, y) = self.corner(corner % self.corners);
                path.line_to(x as f32, y as f32);
            }
            let part = along - f64::from(whole);
            if part > 0.0 {
                let (x0, y0) = self.corner(whole);
                let (x1, y1) = self.corner((whole + 1) % self.corners);
                path.line_to(
                    (x0 + (x1 - x0) * part) as f32,
                    (y0 + (y1 - y0) * part) as f32,
                );
            }
        }
        if along >= f64::from(self.corners) {
            path.close();
        }
        path.finish()
    }
}

fn paint(style: &NoteRollStyle, opacity: f64) -> Paint<'static> {
    let (r, g, b) = style.meter_color.rgb();
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(r, g, b, unit_byte(opacity));
    paint
}

fn fill(frame: &mut Pixmap, path: &SkPath, style: &NoteRollStyle, opacity: f64, at: Transform) {
    frame.fill_path(path, &paint(style, opacity), FillRule::Winding, at, None);
}

/// A dark copy of `path` a little below it, so a shape stays legible on a bright background.
fn shadow(frame: &mut Pixmap, path: &SkPath, offset: f64) {
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(0, 0, 0, 90);
    let at = Transform::from_translate(0.0, offset.max(1.0) as f32);
    frame.fill_path(path, &paint, FillRule::Winding, at, None);
}

fn stroke(frame: &mut Pixmap, path: &SkPath, style: &NoteRollStyle, width: f64, opacity: f64) {
    let line = |width: f64| Stroke {
        width: width as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let mut dark = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    dark.set_color_rgba8(0, 0, 0, unit_byte(0.3 * opacity));
    let below = Transform::from_translate(0.0, (width * 0.6).max(1.0) as f32);
    frame.stroke_path(path, &dark, &line(width), below, None);
    frame.stroke_path(
        path,
        &paint(style, opacity),
        &line(width),
        Transform::identity(),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameRenderer, ScoreBeat, ScoreTempo, VideoSize};
    use auris_core::Color;

    /// Two bars of 3/4 at 120, a beat every half second.
    fn score() -> NoteScore {
        let beats = (0..6)
            .map(|index| ScoreBeat {
                start: f64::from(index) * 0.5,
                end: f64::from(index + 1) * 0.5,
                bar: index / 3 + 1,
                beat: index % 3 + 1,
                beats: 3,
            })
            .collect();
        NoteScore {
            tempos: vec![ScoreTempo {
                start: 0.0,
                bpm: 120.0,
            }],
            beats,
            ..NoteScore::default()
        }
    }

    fn style(tempo: bool, beat_counter: bool) -> NoteRollStyle {
        NoteRollStyle {
            size: VideoSize::new(640, 360),
            playhead_line: false,
            background: Color(0x000000),
            chords: false,
            lyrics: false,
            tempo,
            beat_counter,
            ..NoteRollStyle::default()
        }
    }

    fn drawn(tempo: bool, counter: bool, seconds: f64) -> Pixmap {
        let renderer = FrameRenderer::with_background(&score(), &style(tempo, counter), None);
        let mut frame = renderer.canvas();
        renderer.draw(seconds, &mut frame);
        frame
    }

    /// Lit pixels inside a box.
    fn lit(frame: &Pixmap, x: (f64, f64), y: (f64, f64)) -> usize {
        let (width, height) = (frame.width() as f64, frame.height() as f64);
        let xs = x.0.max(0.0) as u32..x.1.min(width) as u32;
        let ys = y.0.max(0.0) as u32..y.1.min(height) as u32;
        ys.flat_map(|y| xs.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| frame.data()[((y * frame.width() + x) * 4) as usize] > 128)
            .count()
    }

    fn shape() -> Shape {
        let short = 360.0;
        let radius = short * RADIUS;
        Shape {
            corners: 3,
            centre: (640.0 - short * MARGIN - radius, short * MARGIN + radius),
            radius,
        }
    }

    /// Brightly lit pixels in a small box round corner `index` of the counter: a dim corner
    /// still to come, over the faint outline, stays below the threshold.
    fn at_corner(frame: &Pixmap, index: u32) -> usize {
        let (x, y) = shape().corner(index);
        let (from, to) = ((x - 4.0) as u32, (x + 4.0) as u32);
        ((y - 4.0) as u32..(y + 4.0) as u32)
            .flat_map(|y| (from..to).map(move |x| (x, y)))
            .filter(|&(x, y)| frame.data()[((y * frame.width() + x) * 4) as usize] > 200)
            .count()
    }

    #[test]
    fn the_counter_is_a_polygon_with_a_corner_for_each_beat() {
        let shape = shape();
        let (x, y) = shape.corner(0);
        assert!(
            (x - shape.centre.0).abs() < 1e-9,
            "the first beat at the top"
        );
        assert!(y < shape.centre.1);
        let (x, _) = shape.corner(1);
        assert!(x > shape.centre.0, "and the second clockwise from it");
        let square = Shape {
            corners: 4,
            ..shape
        };
        let (x, y) = square.corner(2);
        assert!((x - square.centre.0).abs() < 1e-9 && y > square.centre.1);
    }

    #[test]
    fn the_corner_of_the_beat_sounding_is_lit_and_those_to_come_are_dim() {
        // Late in each beat, once the downbeat's flash of the whole outline has passed.
        let first = drawn(false, true, 0.45);
        let third = drawn(false, true, 1.05);
        assert!(at_corner(&first, 0) > 10, "beat one lit");
        assert_eq!(at_corner(&first, 2), 0, "beat three still to come");
        assert!(at_corner(&third, 2) > 10, "and lit when it comes");
        assert!(at_corner(&third, 1) > 10, "beat two stays lit");
        let after = drawn(false, true, 3.5);
        let corner = shape().corner(0);
        assert_eq!(
            lit(&after, (corner.0 - 80.0, corner.0 + 80.0), (0.0, 120.0)),
            0,
            "nothing once the song is over"
        );
    }

    #[test]
    fn the_outline_is_traced_round_as_the_bar_goes_by() {
        let early = drawn(false, true, 0.45);
        let late = drawn(false, true, 1.4);
        let shape = shape();
        // The middle of the edge from beat two to beat three, at the bottom of the triangle.
        let (x1, y1) = shape.corner(1);
        let (x2, y2) = shape.corner(2);
        let (x, y) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
        let edge = |frame: &Pixmap| lit(frame, (x - 3.0, x + 3.0), (y - 3.0, y + 3.0));
        assert_eq!(edge(&early), 0, "not reached at the start of the bar");
        assert!(
            edge(&late) > 3,
            "reached in the third beat: {}",
            edge(&late)
        );
    }

    #[test]
    fn a_bar_of_one_or_two_beats_is_traced_round_a_circle() {
        let shape = Shape {
            corners: 2,
            ..shape()
        };
        assert!(shape.trace(0.0).is_none());
        let half = shape.trace(1.0).unwrap().bounds();
        assert!(
            half.left() as f64 >= shape.centre.0 - 0.5,
            "the right half first"
        );
        let whole = shape.trace(2.0).unwrap().bounds();
        assert!((whole.width() as f64 - 2.0 * shape.radius).abs() < 1.0);
    }

    #[test]
    fn the_tempo_is_written_beside_the_counter_or_in_its_place() {
        if system_font().is_none() {
            eprintln!("no font installed; skipping the tempo");
            return;
        }
        let short = 360.0;
        let counter_left = 640.0 - short * MARGIN - 2.0 * short * RADIUS;
        let band = (0.0, short * (MARGIN + 2.0 * RADIUS) + 4.0);
        let both = drawn(true, true, 0.25);
        let tempo_only = drawn(true, false, 0.25);
        let counter_only = drawn(false, true, 0.25);
        let left_of_counter = |frame: &Pixmap| lit(frame, (200.0, counter_left - 2.0), band);
        assert!(left_of_counter(&both) > 50, "the tempo beside the counter");
        assert_eq!(left_of_counter(&counter_only), 0, "and none when it is off");
        assert!(
            lit(&tempo_only, (counter_left, 640.0), band) > 50,
            "alone, it takes the corner"
        );
        assert_eq!(
            lit(&drawn(false, false, 0.25), (0.0, 640.0), (0.0, 360.0)),
            0,
            "both off draws nothing"
        );
    }

    #[test]
    fn tempos_are_written_whole_or_to_a_tenth() {
        assert_eq!(tempo_text(120.0), "120");
        assert_eq!(tempo_text(92.5), "92.5");
        assert_eq!(tempo_text(139.98), "140");
    }
}
