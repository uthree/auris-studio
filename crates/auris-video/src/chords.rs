//! The chord overlay: the chord sounding now, the one coming next, and how far through the
//! current one the music is.
//!
//! tiny-skia draws shapes, not text, so the names are outlined from a font already installed on
//! the machine — the same kind of lookup the SVG renderer makes, and like ffmpeg, nothing Auris
//! carries. Each distinct name is outlined once when a renderer is made, so a frame only fills
//! paths it already has. A machine with no font at all draws no chords rather than failing the
//! export: the notes are the video, the names are a caption.

use std::collections::HashMap;
use std::sync::OnceLock;

use tiny_skia::{FillRule, Paint, Path as SkPath, PathBuilder, Pixmap, Rect, Transform};

use crate::{NoteRollStyle, NoteScore};

/// Height of the current chord's text, as a fraction of the frame's shorter side.
const CURRENT_SIZE: f64 = 0.075;
/// The next chord's text, as a fraction of the current chord's.
const NEXT_SCALE: f64 = 0.55;
/// Opacity of the next chord.
const NEXT_OPACITY: f64 = 0.55;
/// Space from the frame's top and left edges, as a fraction of its shorter side.
const MARGIN: f64 = 0.045;
/// How long a new chord takes to fade in, in seconds.
const FADE_SECONDS: f64 = 0.12;
/// How dim a chord is at the instant it changes, before it has faded in.
const FADE_FROM: f64 = 0.35;

/// Families tried in order: the platforms' own interface faces, then what a Linux machine has.
const FAMILIES: [&str; 6] = [
    "Helvetica Neue",
    "Segoe UI",
    "Arial",
    "Inter",
    "DejaVu Sans",
    "Noto Sans",
];

/// An installed font, read once per process.
struct FontData {
    bytes: Vec<u8>,
    index: u32,
}

/// The font chord names are drawn in, if the machine has any.
///
/// Loading every installed face takes a noticeable moment, and a preview makes a renderer on
/// every change of colour, so the answer is kept for the life of the process.
fn system_font() -> Option<&'static FontData> {
    static FONT: OnceLock<Option<FontData>> = OnceLock::new();
    FONT.get_or_init(|| {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let mut families: Vec<_> = FAMILIES
            .iter()
            .map(|name| fontdb::Family::Name(name))
            .collect();
        families.push(fontdb::Family::SansSerif);
        let id = database.query(&fontdb::Query {
            families: &families,
            weight: fontdb::Weight::BOLD,
            ..fontdb::Query::default()
        })?;
        database.with_face_data(id, |data, index| FontData {
            bytes: data.to_vec(),
            index,
        })
    })
    .as_ref()
}

/// A chord name outlined in font units: x from the left of the name, y up from the baseline.
struct Label {
    path: Option<SkPath>,
    advance: f64,
}

/// The overlay for one score, with every name it will show already outlined.
pub(crate) struct ChordOverlay {
    labels: HashMap<String, Label>,
    /// Height of a capital in font units, which is what the text is sized by.
    cap: f64,
}

impl ChordOverlay {
    /// The overlay for `score`, or `None` when it has no chords or the machine has no font.
    pub(crate) fn new(score: &NoteScore) -> Option<Self> {
        if score.chords.is_empty() {
            return None;
        }
        let font = system_font()?;
        let face = ttf_parser::Face::parse(&font.bytes, font.index).ok()?;
        let cap = face
            .capital_height()
            .filter(|height| *height > 0)
            .map_or(f64::from(face.units_per_em()) * 0.7, f64::from);
        let mut labels = HashMap::new();
        for chord in &score.chords {
            labels
                .entry(chord.name.clone())
                .or_insert_with(|| outline(&face, &chord.name));
        }
        Some(Self { labels, cap })
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
        let cap = short * CURRENT_SIZE;
        let margin = short * MARGIN;
        let baseline = margin + cap;
        let ahead = f64::from(style.seconds_visible);

        let current = score.chord_at(seconds);
        let following = score.chords.partition_point(|chord| chord.start <= seconds);
        // The next chord is shown once it is in view, which is when the notes that make it are.
        let next = score
            .chords
            .get(following)
            .filter(|chord| chord.start - seconds <= ahead);

        let mut x = margin;
        if let Some(index) = current {
            let chord = &score.chords[index];
            let fade = ((seconds - chord.start) / FADE_SECONDS).clamp(0.0, 1.0);
            let opacity = FADE_FROM + (1.0 - FADE_FROM) * fade;
            let drawn = self.text(&chord.name, x, baseline, cap, opacity, style, frame);

            // How far through the chord the music is, as a bar under its name: the change is
            // coming when the bar is full.
            let along = ((seconds - chord.start) / (chord.end - chord.start)).clamp(0.0, 1.0);
            let bar_width = drawn.max(cap * 1.2);
            let bar_height = (cap * 0.06).max(2.0);
            let bar_y = baseline + cap * 0.22;
            rect(frame, style, x, bar_y, bar_width, bar_height, 0.25);
            rect(frame, style, x, bar_y, bar_width * along, bar_height, 0.9);
            x += drawn + cap * 0.45;
        }
        if let Some(chord) = next {
            let small = cap * NEXT_SCALE;
            // A small arrow ahead of the name, drawn rather than typed so no font has to have it.
            let arrow = small * 0.62;
            let middle = baseline - small / 2.0;
            let mut path = PathBuilder::new();
            path.move_to(x as f32, (middle - arrow / 2.0) as f32);
            path.line_to((x + arrow * 0.8) as f32, middle as f32);
            path.line_to(x as f32, (middle + arrow / 2.0) as f32);
            path.close();
            if let Some(path) = path.finish() {
                fill(frame, &path, style, NEXT_OPACITY, Transform::identity());
            }
            x += arrow * 0.8 + small * 0.3;
            self.text(&chord.name, x, baseline, small, NEXT_OPACITY, style, frame);
        }
    }

    /// Draws `name` with its baseline at `baseline`, capitals `cap` pixels tall, over a soft
    /// shadow that keeps it legible on a bright background. Returns how wide it was.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &self,
        name: &str,
        x: f64,
        baseline: f64,
        cap: f64,
        opacity: f64,
        style: &NoteRollStyle,
        frame: &mut Pixmap,
    ) -> f64 {
        let Some(label) = self.labels.get(name) else {
            return 0.0;
        };
        let scale = cap / self.cap;
        if let Some(path) = &label.path {
            let place = |dx: f64, dy: f64| {
                Transform::from_row(
                    scale as f32,
                    0.0,
                    0.0,
                    -scale as f32,
                    (x + dx) as f32,
                    (baseline + dy) as f32,
                )
            };
            let shadow = (cap * 0.05).max(1.0);
            let mut paint = Paint {
                anti_alias: true,
                ..Paint::default()
            };
            paint.set_color_rgba8(0, 0, 0, unit_byte(0.45 * opacity));
            frame.fill_path(path, &paint, FillRule::Winding, place(0.0, shadow), None);
            fill(frame, path, style, opacity, place(0.0, 0.0));
        }
        label.advance * scale
    }

    /// How wide `name` is drawn with capitals `cap` pixels tall.
    #[cfg(test)]
    fn text_width(&self, name: &str, cap: f64) -> f64 {
        self.labels
            .get(name)
            .map_or(0.0, |label| label.advance * cap / self.cap)
    }
}

fn rect(
    frame: &mut Pixmap,
    style: &NoteRollStyle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    opacity: f64,
) {
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, width as f32, height as f32)
        .filter(|rect| rect.width() > 0.0 && rect.height() > 0.0)
    {
        // tiny-skia 0.11's AA rectangle fast path asserts when a narrow bar straddles a
        // pixel boundary with no whole pixel inside. The path rasteriser handles that coverage.
        let path = PathBuilder::from_rect(rect);
        fill(frame, &path, style, opacity, Transform::identity());
    }
}

fn fill(frame: &mut Pixmap, path: &SkPath, style: &NoteRollStyle, opacity: f64, at: Transform) {
    let (r, g, b) = style.playhead_color.rgb();
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color_rgba8(r, g, b, unit_byte(opacity));
    frame.fill_path(path, &paint, FillRule::Winding, at, None);
}

fn unit_byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Outlines `name` in font units, with the flat and sharp signs where the font has them.
///
/// In a chord symbol `b` and `#` are only ever accidentals — no suffix spells a word with a `b`
/// in it — so both can be swapped for the real signs without reading the symbol.
fn outline(face: &ttf_parser::Face<'_>, name: &str) -> Label {
    let mut builder = Outline {
        path: PathBuilder::new(),
        x: 0.0,
    };
    for character in name.chars() {
        let sign = match character {
            'b' => Some('♭'),
            '#' => Some('♯'),
            _ => None,
        };
        let glyph = sign
            .and_then(|sign| face.glyph_index(sign))
            .or_else(|| face.glyph_index(character));
        let Some(glyph) = glyph else {
            builder.x += f32::from(face.units_per_em()) * 0.3;
            continue;
        };
        face.outline_glyph(glyph, &mut builder);
        builder.x += f32::from(face.glyph_hor_advance(glyph).unwrap_or(0));
    }
    Label {
        advance: f64::from(builder.x),
        path: builder.path.finish(),
    }
}

/// Collects glyph outlines into one path, each glyph moved along by the advances before it.
struct Outline {
    path: PathBuilder,
    x: f32,
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.x + x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.x + x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path.quad_to(self.x + x1, y1, self.x + x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path
            .cubic_to(self.x + x1, y1, self.x + x2, y2, self.x + x, y);
    }

    fn close(&mut self) {
        self.path.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameRenderer, ScoreChord, VideoSize};
    use auris_core::Color;

    fn score() -> NoteScore {
        let chord = |start: f64, end: f64, name: &str| ScoreChord {
            start,
            end,
            name: name.into(),
        };
        NoteScore {
            chords: vec![
                chord(0.0, 2.0, "C"),
                chord(2.0, 4.0, "Am7"),
                chord(6.0, 8.0, "F"),
            ],
            ..NoteScore::default()
        }
    }

    fn style(chords: bool) -> NoteRollStyle {
        NoteRollStyle {
            size: VideoSize::new(640, 360),
            seconds_visible: 4.0,
            playhead_line: false,
            background: Color(0x000000),
            playhead_color: Color(0xffffff),
            chords,
            ..NoteRollStyle::default()
        }
    }

    /// Lit pixels in the columns `from..to` of the top third, where the overlay sits.
    fn lit(frame: &Pixmap, from: u32, to: u32) -> usize {
        (0..frame.height() / 3)
            .flat_map(|y| (from..to.min(frame.width())).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let at = ((y * frame.width() + x) * 4) as usize;
                frame.data()[at] > 128
            })
            .count()
    }

    fn drawn(chords: bool, seconds: f64) -> Option<Pixmap> {
        system_font()?;
        let renderer = FrameRenderer::with_background(&score(), &style(chords), None);
        let mut frame = renderer.canvas();
        renderer.draw(seconds, &mut frame);
        Some(frame)
    }

    #[test]
    fn a_subpixel_progress_bar_covers_both_sides_of_a_pixel_boundary() {
        let mut frame = Pixmap::new(16, 16).unwrap();
        rect(&mut frame, &style(true), 4.5, 4.5, 1.0, 3.0, 1.0);
        let alpha = |x: usize| frame.data()[(5 * 16 + x) * 4 + 3];
        assert!(alpha(4) > 0 && alpha(5) > 0);
        assert!(alpha(4) < 255 && alpha(5) < 255);
        assert_eq!(alpha(3), 0);
        assert_eq!(alpha(6), 0);
    }

    #[test]
    fn the_first_video_frames_draw_a_narrow_chord_progress_bar() {
        if system_font().is_none() {
            return;
        }
        let renderer = FrameRenderer::with_background(&score(), &NoteRollStyle::default(), None);
        let mut frame = renderer.canvas();
        for index in 0..4 {
            renderer.draw(f64::from(index) / 60.0, &mut frame);
        }
        assert!(lit(&frame, 0, 200) > 0);
    }

    #[test]
    fn the_current_chord_is_written_in_the_corner_with_the_next_beside_it() {
        let Some(frame) = drawn(true, 1.0) else {
            eprintln!("no font installed; skipping the overlay");
            return;
        };
        let overlay = ChordOverlay::new(&score()).unwrap();
        let cap = 360.0 * CURRENT_SIZE;
        let margin = (360.0 * MARGIN) as u32;
        let current = overlay.text_width("C", cap) as u32;
        assert!(
            lit(&frame, margin, margin + current) > 50,
            "the current chord is drawn"
        );
        assert!(
            lit(&frame, margin + current + 10, 640) > 20,
            "and the next one after it"
        );
        assert_eq!(
            lit(&frame, 0, margin.saturating_sub(2)),
            0,
            "inside the margin"
        );
        assert_eq!(
            lit(&drawn(false, 1.0).unwrap(), 0, 640),
            0,
            "and none when off"
        );
    }

    #[test]
    fn a_gap_shows_only_what_is_coming_and_the_end_shows_nothing() {
        let Some(gap) = drawn(true, 5.0) else {
            return;
        };
        let full = drawn(true, 1.0).unwrap();
        let shown = lit(&gap, 0, 640);
        assert!(shown > 10, "the next chord is in view: {shown}");
        assert!(shown < lit(&full, 0, 640), "and smaller than a current one");
        assert_eq!(
            lit(&drawn(true, 9.0).unwrap(), 0, 640),
            0,
            "nothing after the last"
        );
        assert_eq!(
            lit(&drawn(true, 1.0).unwrap(), 0, 640),
            lit(&full, 0, 640),
            "a frame is the same however often it is drawn"
        );
    }

    #[test]
    fn the_bar_under_a_chord_fills_as_it_plays() {
        let (Some(early), Some(late)) = (drawn(true, 0.2), drawn(true, 1.8)) else {
            return;
        };
        let overlay = ChordOverlay::new(&score()).unwrap();
        let cap = 360.0 * CURRENT_SIZE;
        let margin = 360.0 * MARGIN;
        let row = (margin + cap + cap * 0.22 + 1.0) as u32;
        let bright = |frame: &Pixmap| {
            (0..640)
                .filter(|&x| frame.data()[((row * 640 + x) * 4) as usize] > 200)
                .count()
        };
        let width = overlay.text_width("C", cap).max(cap * 1.2);
        assert!(bright(&early) < (width * 0.2) as usize);
        assert!(bright(&late) > (width * 0.8) as usize);
    }

    #[test]
    fn flats_and_sharps_are_outlined_for_every_name() {
        let Some(font) = system_font() else {
            return;
        };
        let face = ttf_parser::Face::parse(&font.bytes, font.index).unwrap();
        for name in ["Bb", "F#m7b5", "Eb/G", "Cmaj7"] {
            let label = outline(&face, name);
            assert!(label.path.is_some(), "{name} has an outline");
            assert!(label.advance > 0.0);
        }
    }
}
